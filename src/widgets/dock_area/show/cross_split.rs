//! Cross-split transposition.
//!
//! A split whose two children are themselves splits of the opposite orientation can end up
//! looking like a 2x2 grid: two perpendicular divider lines crossing at a single point ("+"),
//! rather than an offset/staggered ("L"-shaped) pair of dividers. That layout is ambiguous —
//! the exact same four rectangles can be produced by either grouping (rows-of-columns or
//! columns-of-rows) — so when it happens we draw a small toggle button at the crossing point
//! that swaps between the two representations without moving a single pixel on screen.

use egui::{
    CursorIcon, LayerId, Order, Pos2, Rect, Sense, StrokeKind, Ui, UiBuilder, epaint::CornerRadius,
    pos2, vec2,
};

use crate::dock_area::events::DockEvent;
use crate::{DockArea, Node, NodePath, SeparatorStyle, SplitNode};

/// A detected 2x2 "cross" split: `outer` has two children `c0`/`c1`, each itself a split of
/// the opposite orientation with two children, whose inner dividers are collinear (within
/// [`Self::TOLERANCE`]), forming a single "+".
///
/// `a`/`b` are `c0`'s children (first/second) and `c`/`d` are `c1`'s.
struct CrossSplit {
    outer: NodePath,
    /// Orientation of `outer` itself: `true` if [`Node::Horizontal`].
    outer_horizontal: bool,
    c0: NodePath,
    c1: NodePath,
    a: NodePath,
    b: NodePath,
    c: NodePath,
    d: NodePath,
    /// Position of the divider between `c0` and `c1`, along `outer`'s own axis.
    outer_pos: f32,
    /// Position of the (collinear) divider between `a`/`b` and between `c`/`d`, along the
    /// perpendicular axis.
    inner_pos: f32,
}

impl CrossSplit {
    /// Pixel tolerance for the two inner dividers to be considered collinear.
    const TOLERANCE: f32 = 1.0;

    /// Where the toggle button should be centered: the point where the two dividers cross.
    fn center(&self) -> Pos2 {
        if self.outer_horizontal {
            pos2(self.outer_pos, self.inner_pos)
        } else {
            pos2(self.inner_pos, self.outer_pos)
        }
    }
}

impl<Tab> DockArea<'_, Tab> {
    /// If `outer` is a split whose two children are themselves splits of the opposite
    /// orientation, and the resulting 2x2 layout forms a perfect "+", describe it.
    ///
    /// `outer` must already be known to be a split (parent) node; callers of `show_separator`
    /// establish that before this runs.
    fn detect_cross_split(&self, outer: NodePath) -> Option<CrossSplit> {
        let outer_horizontal = self.dock_state[outer].is_horizontal();
        let [c0, c1] = self.child_paths(outer);

        let inner_horizontal = !outer_horizontal;
        let (a, b) = self.split_pair(c0, inner_horizontal)?;
        let (c, d) = self.split_pair(c1, inner_horizontal)?;

        let rect = |p: NodePath| self.layout.rect(p);
        let a_rect = rect(a)?;
        let b_rect = rect(b)?;
        let c_rect = rect(c)?;
        let d_rect = rect(d)?;
        let c0_rect = rect(c0)?;
        let c1_rect = rect(c1)?;

        // A degenerate (zero-size) quadrant has nothing meaningful to pivot around, and
        // dividing by its size below would produce a NaN fraction — which this crate has
        // already been bitten by once (see the fork's incident notes on `SplitNode.fraction`).
        for r in [a_rect, b_rect, c_rect, d_rect] {
            if r.width() <= 0.0 || r.height() <= 0.0 {
                return None;
            }
        }

        // Position of the inner divider (between `a`/`b`, and between `c`/`d`), measured along
        // the axis perpendicular to `outer`'s own (i.e. the axis `inner_horizontal` splits on).
        let inner_pos_of = |first: Rect, second: Rect| -> f32 {
            if inner_horizontal {
                egui::lerp(first.right()..=second.left(), 0.5)
            } else {
                egui::lerp(first.bottom()..=second.top(), 0.5)
            }
        };
        let pos0 = inner_pos_of(a_rect, b_rect);
        let pos1 = inner_pos_of(c_rect, d_rect);

        // The two inner dividers must be collinear for the whole thing to look like a single
        // "+" rather than a staggered "L".
        if (pos0 - pos1).abs() > CrossSplit::TOLERANCE {
            return None;
        }
        let inner_pos = 0.5 * (pos0 + pos1);

        let outer_pos = if outer_horizontal {
            egui::lerp(c0_rect.right()..=c1_rect.left(), 0.5)
        } else {
            egui::lerp(c0_rect.bottom()..=c1_rect.top(), 0.5)
        };

        Some(CrossSplit {
            outer,
            outer_horizontal,
            c0,
            c1,
            a,
            b,
            c,
            d,
            outer_pos,
            inner_pos,
        })
    }

    /// If `path` is a split node of the given orientation, its two children (first, second).
    fn split_pair(&self, path: NodePath, horizontal: bool) -> Option<(NodePath, NodePath)> {
        let node = &self.dock_state[path];
        let matches = if horizontal {
            node.is_horizontal()
        } else {
            node.is_vertical()
        };
        matches.then(|| {
            let [left, right] = self.child_paths(path);
            (left, right)
        })
    }

    /// If `outer` and its children form a cross split (see [`CrossSplit`]), draw a small
    /// toggle button at the crossing point and, on click, transpose the grouping.
    ///
    /// Drawn in its own [`Order::Foreground`] layer, on top of the surrounding separators'
    /// default layer. Layer order is how egui resolves overlapping widgets — a widget in a
    /// higher-order layer wins hover/click over anything underneath regardless of screen
    /// position or interact() call order — so this is what stops the resize cursor and the
    /// separator's own hover highlight from "bubbling" through the button on top of it, with
    /// no need to carve a hole out of the separator's interact rect.
    pub(super) fn draw_cross_split_toggle(
        &mut self,
        ui: &mut Ui,
        outer: NodePath,
        style: &SeparatorStyle,
    ) {
        if !self.show_cross_split_toggle {
            return;
        }
        let Some(cross) = self.detect_cross_split(outer) else {
            return;
        };
        let center = cross.center();

        let button_size = (style.width + style.extra_interact_width).max(14.0);
        let button_rect = Rect::from_center_size(center, vec2(button_size, button_size));
        let button_id = ui.id().with((outer.node, "cross_split_toggle"));
        let layer_id = LayerId::new(Order::Foreground, button_id);

        let response = ui
            .scope_builder(UiBuilder::new().layer_id(layer_id), |ui| {
                let response = ui
                    .interact(button_rect, button_id, Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand);

                let color = if response.hovered() {
                    style.color_hovered
                } else {
                    style.color_idle
                };
                ui.painter().rect(
                    button_rect,
                    CornerRadius::from(button_size * 0.25),
                    color,
                    egui::Stroke::NONE,
                    StrokeKind::Inside,
                );
                // A small pinwheel of four arrows pointing outward, hinting at "swap the
                // grouping":
                let stroke = egui::Stroke::new(1.5, style.color_dragged);
                let arm = button_size * 0.28;
                for angle_deg in [45.0_f32, 135.0, 225.0, 315.0] {
                    let angle = angle_deg.to_radians();
                    let dir_vec = vec2(angle.cos(), angle.sin());
                    let tip = center + dir_vec * arm;
                    let base = center + dir_vec * (arm * 0.3);
                    ui.painter().line_segment([base, tip], stroke);
                    let perp = vec2(-dir_vec.y, dir_vec.x) * (arm * 0.25);
                    ui.painter()
                        .line_segment([tip, tip - dir_vec * (arm * 0.35) + perp], stroke);
                    ui.painter()
                        .line_segment([tip, tip - dir_vec * (arm * 0.35) - perp], stroke);
                }

                response
            })
            .inner;

        if response.clicked() {
            self.transpose_cross_split(&cross);
            self.events.push(DockEvent::CrossSplitTransposed);
        }
    }

    /// Perform the actual tree surgery for [`Self::draw_cross_split_toggle`]: transpose the
    /// row/column grouping of a 2x2 cross split while keeping every leaf exactly where it is
    /// on screen.
    ///
    /// Before (`outer_horizontal`): `outer` = Horizontal(`c0`, `c1`), `c0` = Vertical(`a`, `b`),
    /// `c1` = Vertical(`c`, `d`) — so on screen: `a`|`c` on top, `b`|`d` on bottom, with `a`,`b`
    /// in the left column and `c`,`d` in the right column.
    ///
    /// After: `outer` = Vertical(`c0`, `c1`), `c0` = Horizontal(`a`, `c`) (the top row), `c1` =
    /// Horizontal(`b`, `d`) (the bottom row). Same four rectangles, different grouping. The
    /// vertical-outer case is the mirror image of this.
    fn transpose_cross_split(&mut self, cross: &CrossSplit) {
        let &CrossSplit {
            outer,
            outer_horizontal,
            c0,
            c1,
            a,
            b,
            c,
            d,
            ..
        } = cross;

        // The four leaves, named by their (unchanging) position on screen. `a`/`b` are `c0`'s
        // children and `c`/`d` are `c1`'s — which maps to screen quadrants differently
        // depending on whether `c0`/`c1` were the left/right columns (outer horizontal) or the
        // top/bottom rows (outer vertical).
        let (top_left, top_right, bottom_left, bottom_right) = if outer_horizontal {
            (a, c, b, d)
        } else {
            (a, b, c, d)
        };

        // Derive the new fractions straight from the *actual current pixel sizes* of the four
        // leaves, not from divider positions: each one's rect is already exactly what the
        // layout pass produced for its (separator-subtracted) available space, so re-feeding
        // those same sizes back in as a fraction reproduces the identical split. Going through
        // divider midpoints and the outer rect instead would silently fold half a separator
        // width into each side and drift the ratio (most visible on small tiles). The two
        // leaves sharing a row/column can differ by up to `CrossSplit::TOLERANCE` px (that is
        // the point of the tolerance), so average the pair.
        let rect = |p: NodePath| {
            self.layout
                .rect(p)
                .expect("cross split leaves were laid out this frame")
        };
        let top_h = 0.5 * (rect(top_left).height() + rect(top_right).height());
        let bottom_h = 0.5 * (rect(bottom_left).height() + rect(bottom_right).height());
        let left_w = 0.5 * (rect(top_left).width() + rect(bottom_left).width());
        let right_w = 0.5 * (rect(top_right).width() + rect(bottom_right).width());

        let row_fraction = top_h / (top_h + bottom_h);
        let col_fraction = left_w / (left_w + right_w);

        let new_outer_horizontal = !outer_horizontal;

        let (new_c0, new_c1) = if new_outer_horizontal {
            // `c0` becomes the left column: Vertical(top_left, bottom_left).
            // `c1` becomes the right column: Vertical(top_right, bottom_right).
            (
                Node::Vertical(SplitNode::new(
                    [top_left.node, bottom_left.node],
                    row_fraction,
                )),
                Node::Vertical(SplitNode::new(
                    [top_right.node, bottom_right.node],
                    row_fraction,
                )),
            )
        } else {
            // `c0` becomes the top row: Horizontal(top_left, top_right).
            // `c1` becomes the bottom row: Horizontal(bottom_left, bottom_right).
            (
                Node::Horizontal(SplitNode::new(
                    [top_left.node, top_right.node],
                    col_fraction,
                )),
                Node::Horizontal(SplitNode::new(
                    [bottom_left.node, bottom_right.node],
                    col_fraction,
                )),
            )
        };

        let new_outer = if new_outer_horizontal {
            Node::Horizontal(SplitNode::new([c0.node, c1.node], col_fraction))
        } else {
            Node::Vertical(SplitNode::new([c0.node, c1.node], row_fraction))
        };

        self.dock_state[outer] = new_outer;
        self.dock_state[c0] = new_c0;
        self.dock_state[c1] = new_c1;
    }
}

#[cfg(test)]
mod tests {
    use egui::{CentralPanel, Context, Id, RawInput, Ui, Vec2, WidgetText};
    use proptest::prelude::*;

    use super::*;
    use crate::layout::DockLayout;
    use crate::{DockState, NodeId, Split, Style, SurfaceIndex, TabViewer};

    const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
    const DOCK_ID: &str = "cross_split_test_dock";

    struct Viewer;

    impl TabViewer for Viewer {
        type Tab = u32;

        fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
            tab.to_string().into()
        }

        fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
            ui.label(tab.to_string());
        }
    }

    /// Build a 2x2 cross: split the root into two halves along `outer_horizontal`'s axis,
    /// then split each half again in the opposite direction using the *same*
    /// `inner_fraction`. Since both halves always span the outer split's full cross-axis
    /// extent, sharing the inner fraction guarantees their inner dividers land at the exact
    /// same position once laid out — a perfect "+", by construction.
    ///
    /// Returns the state, the outer split's id, and the four leaves named by their
    /// (unchanging) screen quadrant: `[top_left, bottom_left, top_right, bottom_right]`.
    fn build_cross(
        outer_horizontal: bool,
        outer_fraction: f32,
        inner_fraction: f32,
    ) -> (DockState<u32>, NodeId, [NodeId; 4]) {
        let mut state = DockState::new(vec![0u32]);
        let a = state.main_surface().root().unwrap();

        let (outer_split, inner_split) = if outer_horizontal {
            (Split::Right, Split::Below)
        } else {
            (Split::Below, Split::Right)
        };

        let [_, node_c] = state.split(
            NodePath::new(SurfaceIndex::main(), a),
            outer_split,
            outer_fraction,
            Node::leaf(2u32),
        );
        let outer = state.main_surface().root().unwrap();

        let [_, node_b] = state.split(
            NodePath::new(SurfaceIndex::main(), a),
            inner_split,
            inner_fraction,
            Node::leaf(1u32),
        );
        let [_, node_d] = state.split(
            NodePath::new(SurfaceIndex::main(), node_c),
            inner_split,
            inner_fraction,
            Node::leaf(3u32),
        );

        (state, outer, [a, node_b, node_c, node_d])
    }

    /// Runs one real headless frame of the dock and leaves its geometry in `ctx` memory,
    /// exactly as `DockArea::show_inside_with_response` normally would inside a real app.
    fn render(ctx: &Context, state: &mut DockState<u32>, style: &Style, id: Id) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                DockArea::new(state)
                    .id(id)
                    .style(style.clone())
                    .show_inside_with_response(ui, &mut Viewer);
            });
        });
        // Headless harness, no GPU backend to hand the delta to.
        output.textures_delta.clear();
    }

    fn leaf_rects(layout: &DockLayout, leaves: [NodeId; 4]) -> [Rect; 4] {
        leaves.map(|id| {
            layout
                .rect(NodePath::new(SurfaceIndex::main(), id))
                .expect("leaf was laid out this frame")
        })
    }

    fn assert_rects_close(before: [Rect; 4], after: [Rect; 4]) {
        for (b, a) in before.iter().zip(after.iter()) {
            assert!(
                (b.min - a.min).length() < 0.1 && (b.max - a.max).length() < 0.1,
                "leaf rect moved by the toggle: {b:?} -> {a:?}"
            );
        }
    }

    #[test]
    fn detects_perfect_cross_horizontal() {
        let (mut state, outer, _) = build_cross(true, 0.5, 0.5);
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        render(&ctx, &mut state, &Style::default(), id);

        let mut area = DockArea::new(&mut state).id(id);
        area.layout = DockLayout::load(&ctx, id);
        assert!(
            area.detect_cross_split(NodePath::new(SurfaceIndex::main(), outer))
                .is_some()
        );
    }

    #[test]
    fn detects_perfect_cross_vertical() {
        let (mut state, outer, _) = build_cross(false, 0.5, 0.5);
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        render(&ctx, &mut state, &Style::default(), id);

        let mut area = DockArea::new(&mut state).id(id);
        area.layout = DockLayout::load(&ctx, id);
        assert!(
            area.detect_cross_split(NodePath::new(SurfaceIndex::main(), outer))
                .is_some()
        );
    }

    /// Two halves whose internal dividers land at different offsets ("L"-shaped, staggered)
    /// must NOT be offered the toggle — there is no single crossing point to pivot around.
    #[test]
    fn rejects_staggered_l_shape() {
        let mut state = DockState::new(vec![0u32]);
        let a = state.main_surface().root().unwrap();
        let [_, node_c] = state.split(
            NodePath::new(SurfaceIndex::main(), a),
            Split::Right,
            0.5,
            Node::leaf(2u32),
        );
        let outer = state.main_surface().root().unwrap();
        state.split(
            NodePath::new(SurfaceIndex::main(), a),
            Split::Below,
            0.2,
            Node::leaf(1u32),
        );
        state.split(
            NodePath::new(SurfaceIndex::main(), node_c),
            Split::Below,
            0.8,
            Node::leaf(3u32),
        );

        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        render(&ctx, &mut state, &Style::default(), id);

        let mut area = DockArea::new(&mut state).id(id);
        area.layout = DockLayout::load(&ctx, id);
        assert!(
            area.detect_cross_split(NodePath::new(SurfaceIndex::main(), outer))
                .is_none()
        );
    }

    /// A split whose children aren't both opposite-orientation splits (e.g. one side is
    /// still a plain leaf) must never be mistaken for a cross.
    #[test]
    fn rejects_non_split_children() {
        let mut state = DockState::new(vec![0u32]);
        let a = state.main_surface().root().unwrap();
        let outer_split_target = NodePath::new(SurfaceIndex::main(), a);
        state.split(outer_split_target, Split::Right, 0.5, Node::leaf(1u32));
        let outer = state.main_surface().root().unwrap();

        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        render(&ctx, &mut state, &Style::default(), id);

        let mut area = DockArea::new(&mut state).id(id);
        area.layout = DockLayout::load(&ctx, id);
        // Both children are plain leaves here, not splits at all.
        assert!(
            area.detect_cross_split(NodePath::new(SurfaceIndex::main(), outer))
                .is_none()
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// The whole point of the feature: toggling a cross split must not move a single
        /// pixel on screen, and toggling back must restore the original grouping.
        #[test]
        fn transpose_preserves_leaf_rects_and_round_trips(
            outer_horizontal in any::<bool>(),
            outer_fraction in 0.05f32..0.95,
            inner_fraction in 0.05f32..0.95,
        ) {
            let (mut state, outer_id, leaves) = build_cross(outer_horizontal, outer_fraction, inner_fraction);
            let ctx = Context::default();
            let id = Id::new(DOCK_ID);
            let style = Style::default();
            let outer = NodePath::new(SurfaceIndex::main(), outer_id);

            render(&ctx, &mut state, &style, id);
            let before = leaf_rects(&DockLayout::load(&ctx, id), leaves);

            {
                let mut area = DockArea::new(&mut state).id(id);
                area.layout = DockLayout::load(&ctx, id);
                let cross = area
                    .detect_cross_split(outer)
                    .expect("must be a perfect cross by construction");
                area.transpose_cross_split(&cross);
            }

            // Re-render with the *same* screen: the transposed tree must reproduce it exactly.
            render(&ctx, &mut state, &style, id);
            let after = leaf_rects(&DockLayout::load(&ctx, id), leaves);
            assert_rects_close(before, after);

            prop_assert_eq!(
                state[outer].is_horizontal(),
                !outer_horizontal,
                "orientation did not flip"
            );

            // Toggling back must restore both the original geometry and the original grouping.
            {
                let mut area = DockArea::new(&mut state).id(id);
                area.layout = DockLayout::load(&ctx, id);
                let cross2 = area
                    .detect_cross_split(outer)
                    .expect("still a perfect cross after one toggle");
                area.transpose_cross_split(&cross2);
            }
            render(&ctx, &mut state, &style, id);
            let round_tripped = leaf_rects(&DockLayout::load(&ctx, id), leaves);
            assert_rects_close(before, round_tripped);

            prop_assert_eq!(state[outer].is_horizontal(), outer_horizontal, "orientation not restored");
        }
    }
}
