//! Cross-split transposition.
//!
//! Where the two children of a split are themselves cut by dividers that line up, the dock
//! shows a "+": one divider line running the full extent of both children, crossing the line
//! between them. A "+" is ambiguous — the exact same rectangles are produced by either grouping
//! (rows of columns, or columns of rows) and nothing on screen says which one the tree holds —
//! so at every crossing point we draw a small toggle button that swaps the two representations
//! without moving a single pixel.
//!
//! # Why the law is stated on bands
//!
//! A region cut into three columns is `H(H(C, D), E)` or `H(C, H(D, E))` in the tree depending
//! on nothing but the order the splits were made in; on screen the two are the same picture.
//! So a rule read straight off the tree finds a *different* set of dividers for the same
//! screen: at any fixed depth only one divider of each side is visible, and which one it is
//! comes from the split order rather than from the picture. That is exactly how a "+" that was
//! plainly on screen went unoffered — see `tests::detects_a_cross_where_a_band_has_three_columns`,
//! where the very same screen built in the other order *was* detected.
//!
//! Flattened into a [`Band`] — an ordered list of `n` parts with `n - 1` dividers between them,
//! however the tree happens to have nested them — the model is the picture, and the law reads
//! off it in one line: **a crossing is a position that both neighbouring bands have a divider
//! at**. Neither `n` nor `m` appears in it, and neither does depth.

use std::collections::VecDeque;

use egui::{
    CursorIcon, LayerId, Order, Pos2, Rect, Sense, StrokeKind, Ui, UiBuilder, Vec2,
    epaint::CornerRadius, pos2, vec2,
};

use crate::core::tree::regroup::Regroup;
use crate::dock_area::events::DockEvent;
use crate::{CrossSplitToggleStyle, DockArea, NodeId, NodePath, SeparatorStyle};

/// One side of a split, with the chain of same-orientation splits at its root flattened: `n`
/// parts side by side, with `n - 1` dividers between them.
///
/// The flattening stops at the first node of the *other* orientation, and that is not an
/// approximation — a divider below such a node does not span the band, so it cannot reach
/// either end of it and cannot take part in a crossing.
struct Band {
    /// The parts, in screen order (left to right, or top to bottom).
    parts: Vec<NodePath>,

    /// The `parts.len() - 1` splits of the chain, in screen order: `dividers[k]` is the split
    /// whose boundary falls between `parts[k]` and `parts[k + 1]`.
    ///
    /// They double as the pool of ids a transposition rebuilds the band out of — a chain taken
    /// apart and re-nested needs exactly as many splits as it had.
    dividers: Vec<NodeId>,

    /// The `parts.len() + 1` boundaries along the band's own axis, ascending: the band's two
    /// outer edges, and each divider's midpoint in between.
    ///
    /// Part `k` is cut from the interval `bounds[k]..bounds[k + 1]` — the rectangle
    /// `compute_rect_sizes` hands it *before* insetting half a separator on each inner side. So
    /// `(boundary - group_start) / group_length` taken off these numbers is exactly the fraction
    /// that reproduces what is on screen, with no separator width to fold in or out. Deriving a
    /// fraction from the parts' *sizes* instead drifts the ratio, most visibly on small tiles.
    bounds: Vec<f32>,
}

impl Band {
    /// Where this band's dividers are, in screen coordinates along its axis. Ascending.
    fn divider_positions(&self) -> &[f32] {
        &self.bounds[1..self.bounds.len() - 1]
    }

    /// The band's full extent along its axis.
    fn span(&self) -> (f32, f32) {
        (self.bounds[0], self.bounds[self.bounds.len() - 1])
    }

    /// Whether every part is at least `extra` long — and with it, whether this band can be cut
    /// anywhere and re-nested without a single boundary moving.
    ///
    /// [`SeparatorStyle::extra`](crate::SeparatorStyle::extra) is a margin each child of a split
    /// must keep, so a split whose interval is `R` long can only put its boundary between
    /// `extra` and `R - extra` — and a fraction outside that band is *drawn* clamped (see
    /// `SeparatorBand`). A transposition rebuilds both chains right-leaning, which is the
    /// cheapest nesting there is: split `k` gets the interval "from divider `k` to the end of
    /// its group", so each of its two sides holds at least one whole part. That makes "every
    /// part is at least `extra` long" the exact condition under which every fraction the rebuild
    /// writes is honoured — i.e. under which the promise that no pixel moves is true by
    /// construction rather than by luck.
    ///
    /// It is rarely binding: the same clamp is what *produced* these parts, and it only lets one
    /// out below `extra` where the interval it was cut from was itself shorter than `2 * extra`
    /// — a window squeezed small. There, no button is offered rather than one that jumps.
    fn parts_can_be_renested(&self, extra: f32) -> bool {
        self.bounds
            .windows(2)
            .all(|pair| pair[1] - pair[0] >= extra)
    }
}

/// Every "+" on the line between one split's two children, together with the two bands they
/// were found in — which the transposition needs, so they are kept rather than re-derived.
struct Crossings {
    outer: NodePath,

    /// Orientation of `outer` itself: `true` if [`crate::Node::Horizontal`].
    outer_horizontal: bool,

    /// The two bands, in `outer`'s child order. Both run *across* `outer`'s own axis: a divider
    /// that reaches the line between the children has to.
    bands: [Band; 2],

    /// `outer`'s own three boundaries along its own axis: the far edge of the first band, the
    /// line between the two, and the far edge of the second.
    outer_bounds: [f32; 3],

    /// One entry per crossing: which divider of each band meets here (`k` meaning "between part
    /// `k` and part `k + 1`"), and the point on screen where they cross.
    at: Vec<([usize; 2], Pos2)>,
}

impl Crossings {
    /// Pixel tolerance for two dividers to be considered the same line.
    const TOLERANCE: f32 = 1.0;
}

/// One edge of a rectangle along an axis: `x` for a horizontal one, `y` for a vertical one.
fn edge(rect: Rect, horizontal: bool, far: bool) -> f32 {
    match (horizontal, far) {
        (true, false) => rect.min.x,
        (true, true) => rect.max.x,
        (false, false) => rect.min.y,
        (false, true) => rect.max.y,
    }
}

/// The group `band.parts[from..=to]`, rebuilt as a chain of splits taken from `pool`.
///
/// Right-leaning, and the nesting is genuinely free: every binary arrangement of the same parts
/// at the same boundaries draws the same picture — that ambiguity is the whole reason this
/// module exists. What is not free is the fractions, and those come straight off [`Band::bounds`].
fn rebuild_chain(
    band: &Band,
    from: usize,
    to: usize,
    horizontal: bool,
    pool: &mut impl Iterator<Item = NodeId>,
) -> Regroup {
    if from == to {
        return Regroup::Keep(band.parts[from].node);
    }
    let id = pool
        .next()
        .expect("a chain is rebuilt out of exactly the splits it was taken apart from");
    Regroup::Split {
        id,
        horizontal,
        fraction: (band.bounds[from + 1] - band.bounds[from])
            / (band.bounds[to + 1] - band.bounds[from]),
        children: [
            Box::new(Regroup::Keep(band.parts[from].node)),
            Box::new(rebuild_chain(band, from + 1, to, horizontal, pool)),
        ],
    }
}

/// One half of a transposed cross: what the two bands each had on one side of the crossing
/// line, stacked along `outer`'s old axis.
///
/// The half itself reuses a split from `pool`, and so does every chain inside it, in that order
/// — which is what makes the 2x2 case come out with `outer`'s two old children still playing
/// the two halves, exactly as the special-cased version of this used to leave them.
fn rebuild_half(
    bands: &[Band; 2],
    from: [usize; 2],
    to: [usize; 2],
    outer_horizontal: bool,
    stack_fraction: f32,
    pool: &mut impl Iterator<Item = NodeId>,
) -> Regroup {
    let id = pool
        .next()
        .expect("each half of a transposed cross reuses one of the chains' splits");
    let inner_horizontal = !outer_horizontal;
    Regroup::Split {
        id,
        horizontal: outer_horizontal,
        fraction: stack_fraction,
        children: [
            Box::new(rebuild_chain(
                &bands[0],
                from[0],
                to[0],
                inner_horizontal,
                pool,
            )),
            Box::new(rebuild_chain(
                &bands[1],
                from[1],
                to[1],
                inner_horizontal,
                pool,
            )),
        ],
    }
}

impl<Tab> DockArea<'_, Tab> {
    /// Flattens the chain of `horizontal`-oriented splits rooted at `root` into a [`Band`].
    ///
    /// `None` if the geometry map does not describe every part, or if one of them is degenerate
    /// (zero-size): such a part has nothing to pivot around, and dividing by its extent would
    /// produce a NaN fraction — which this crate has already been bitten by once (see the fork's
    /// incident notes on `SplitNode.fraction`).
    fn band(&self, root: NodePath, horizontal: bool) -> Option<Band> {
        let mut parts = Vec::new();
        let mut dividers = Vec::new();
        self.collect_band(root, horizontal, &mut parts, &mut dividers);

        let rects: Vec<Rect> = parts
            .iter()
            .map(|part| self.layout.rect(*part))
            .collect::<Option<_>>()?;
        if rects.iter().any(|r| r.width() <= 0.0 || r.height() <= 0.0) {
            return None;
        }

        let mut bounds = Vec::with_capacity(parts.len() + 1);
        bounds.push(edge(rects[0], horizontal, false));
        for pair in rects.windows(2) {
            bounds.push(0.5 * (edge(pair[0], horizontal, true) + edge(pair[1], horizontal, false)));
        }
        bounds.push(edge(rects[rects.len() - 1], horizontal, true));

        Some(Band {
            parts,
            dividers,
            bounds,
        })
    }

    /// The in-order walk behind [`Self::band`].
    ///
    /// In-order is what puts both output lists in screen order: everything the first child
    /// contributes lies before the split's own boundary, and everything the second contributes
    /// lies after it, at every level of the chain.
    fn collect_band(
        &self,
        path: NodePath,
        horizontal: bool,
        parts: &mut Vec<NodePath>,
        dividers: &mut Vec<NodeId>,
    ) {
        let node = &self.dock_state[path];
        let in_chain = if horizontal {
            node.is_horizontal()
        } else {
            node.is_vertical()
        };
        if !in_chain {
            parts.push(path);
            return;
        }
        let [first, second] = self.child_paths(path);
        self.collect_band(first, horizontal, parts, dividers);
        dividers.push(path.node);
        self.collect_band(second, horizontal, parts, dividers);
    }

    /// Every "+" on the line between `outer`'s two children.
    ///
    /// `outer` must already be known to be a split (parent) node; callers of `show_separator`
    /// establish that before this runs.
    fn detect_crossings(&self, outer: NodePath, extra: f32) -> Option<Crossings> {
        let outer_horizontal = self.dock_state[outer].is_horizontal();
        let [c0, c1] = self.child_paths(outer);

        // A divider that can reach the line between the two children runs *across* `outer`'s
        // own axis, so it comes from a split of the opposite orientation — which is the
        // orientation each band is flattened along.
        let inner_horizontal = !outer_horizontal;
        let band0 = self.band(c0, inner_horizontal)?;
        let band1 = self.band(c1, inner_horizontal)?;
        if !band0.parts_can_be_renested(extra) || !band1.parts_can_be_renested(extra) {
            return None;
        }

        let (c0_rect, c1_rect) = (self.layout.rect(c0)?, self.layout.rect(c1)?);
        let outer_bounds = [
            edge(c0_rect, outer_horizontal, false),
            0.5 * (edge(c0_rect, outer_horizontal, true) + edge(c1_rect, outer_horizontal, false)),
            edge(c1_rect, outer_horizontal, true),
        ];

        // Both lists ascend, so one merge walk pairs them up: it finds every pair that is the
        // same line and, unlike a nested scan, cannot hand one divider two partners — which
        // would put two buttons on one point the moment two dividers ever sat a pixel apart.
        let (first, second) = (band0.divider_positions(), band1.divider_positions());
        let (mut i, mut j) = (0, 0);
        let mut at = Vec::new();
        while i < first.len() && j < second.len() {
            let gap = first[i] - second[j];
            if gap.abs() <= Crossings::TOLERANCE {
                let line = 0.5 * (first[i] + second[j]);
                let center = if outer_horizontal {
                    pos2(outer_bounds[1], line)
                } else {
                    pos2(line, outer_bounds[1])
                };
                at.push(([i, j], center));
                i += 1;
                j += 1;
            } else if gap < 0.0 {
                i += 1;
            } else {
                j += 1;
            }
        }

        Some(Crossings {
            outer,
            outer_horizontal,
            bands: [band0, band1],
            outer_bounds,
            at,
        })
    }

    /// Draws a toggle button at every crossing on `outer`'s line and, on click, transposes the
    /// grouping around the one that was pressed.
    ///
    /// Each button is drawn in its own [`Order::Foreground`] layer, on top of the surrounding
    /// separators' default layer. Layer order is how egui resolves overlapping widgets — a
    /// widget in a higher-order layer wins hover/click over anything underneath regardless of
    /// screen position or `interact()` call order — so this is what stops the resize cursor and
    /// the separator's own hover highlight from "bubbling" through the button on top of it, with
    /// no need to carve a hole out of the separator's interact rect.
    pub(super) fn draw_cross_split_toggle(
        &mut self,
        ui: &mut Ui,
        outer: NodePath,
        style: &SeparatorStyle,
        toggle: &CrossSplitToggleStyle,
    ) {
        if !self.show_cross_split_toggle {
            return;
        }
        let Some(crossings) = self.detect_crossings(outer, style.extra) else {
            return;
        };

        // Every button is drawn, but at most one may be acted on: a transposition rewrites the
        // tree these crossings were read off, so the rest of them stop describing anything the
        // moment the first one fires.
        let mut pressed = None;
        for (index, &(at, center)) in crossings.at.iter().enumerate() {
            if self.draw_one_toggle(ui, &crossings, at, center, style, toggle) {
                pressed = Some(index);
            }
        }

        if let Some(index) = pressed {
            self.transpose_cross_split(ui.ctx().pixels_per_point(), &crossings, index);
            self.events.push(DockEvent::CrossSplitTransposed);
        }
    }

    /// One toggle button. Returns whether it was clicked this frame.
    ///
    /// # It claims no space
    ///
    /// The button is drawn into a child [`Ui`] made with [`Ui::new_child`], and that is not a
    /// stylistic choice over `ui.scope_builder`: a *scope* ends by folding the child's
    /// `min_rect` into the parent and advancing its cursor past it. The parent here is the `Ui`
    /// the entire dock is drawn into, and in a floating window that `Ui`'s `min_rect` is the
    /// size the window asks for — so each button drawn grew the window by one item spacing,
    /// every frame, and `constrain_to` turned that into a slide up the screen once it reached
    /// the bottom. See `a_cross_in_a_window_does_not_make_the_window_creep`. A button sitting
    /// on a separator occupies a place the layout has already accounted for; it must take none
    /// of its own.
    ///
    /// # It catches the pointer from outside itself
    ///
    /// Two radii, and what they are for is in [`CrossSplitToggleStyle`]: the pointer is caught
    /// from `catch_extra` outside the drawn square, and held until it leaves a zone
    /// `hold_extra` wider than that. Which of the two applies is state — "is this button
    /// currently holding the pointer" — so it is remembered per button between frames.
    fn draw_one_toggle(
        &self,
        ui: &mut Ui,
        crossings: &Crossings,
        at: [usize; 2],
        center: Pos2,
        style: &SeparatorStyle,
        toggle: &CrossSplitToggleStyle,
    ) -> bool {
        // Keyed by the two dividers that meet here rather than by their position in the band:
        // an id has to survive a neighbouring divider being dragged past, and an index does
        // not.
        let button_id = ui.id().with((
            crossings.bands[0].dividers[at[0]],
            crossings.bands[1].dividers[at[1]],
            "cross_split_toggle",
        ));
        let layer_id = LayerId::new(Order::Foreground, button_id);

        let drawn_idle = Rect::from_center_size(center, Vec2::splat(toggle.size));
        let catch_rect = drawn_idle.expand(toggle.catch_extra);

        // The grip is remembered as *when* it was last held, not as a flag. A crossing can stop
        // existing while it holds the pointer — the layout changes under it — and a bare `true`
        // left in memory would arm the button the moment the same two dividers line up again,
        // widening its reach for a frame at a point the pointer merely happens to be near. A
        // pass number cannot go stale: it either names the frame before this one, or it does
        // not.
        let pass = ui.ctx().cumulative_pass_nr();
        let held_on: Option<u64> = ui.data(|data| data.get_temp(button_id));
        let holding = held_on.is_some_and(|last| last + 1 >= pass);
        let hit_rect = if holding {
            catch_rect.expand(toggle.hold_extra)
        } else {
            catch_rect
        };

        let ui = &mut ui.new_child(UiBuilder::new().layer_id(layer_id).max_rect(hit_rect));

        let response = ui
            .interact(hit_rect, button_id, Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);

        // A press keeps its grip even if the pointer slides off, the same way any button does;
        // without it, releasing a hair outside would both cancel the click and disarm.
        let holds_now = response.hovered() || response.is_pointer_button_down_on();
        ui.data_mut(|data| {
            if holds_now {
                data.insert_temp(button_id, pass);
            } else {
                data.remove_temp::<u64>(button_id);
            }
        });

        // Grown to exactly the catch zone while it holds the pointer: the target you can hit is
        // then the target you can see, and the growth itself is the feedback that you have it.
        let button_rect = if holds_now { catch_rect } else { drawn_idle };
        let button_size = button_rect.width();

        let color = if holds_now {
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
        // A small pinwheel of four arrows pointing outward, hinting at "swap the grouping":
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

        response.clicked()
    }

    /// Transposes the grouping around crossing `index`, keeping every leaf exactly where it is
    /// on screen.
    ///
    /// The line through that crossing runs the full extent of both bands, so it can carry the
    /// whole of `outer`. Before (`outer_horizontal`): `outer` groups two side-by-side bands,
    /// each a stack. After: `outer` is cut *by that line* into two side-by-side halves, and each
    /// half stacks what the two bands had on its side of it. The 2x2 case — one divider per
    /// band, both bands two parts — is this with every chain of length one; the vertical-outer
    /// case is the mirror image.
    ///
    /// The four rectangles of a 2x2 are what makes that case look like a swap of two groupings;
    /// in general there are `n + m` of them and the "swap" reading falls away, but the promise
    /// does not: not a pixel moves, and pressing the button at the same point again brings the
    /// original grouping back.
    ///
    /// This runs in the *middle* of the separator pass (its only caller is a toggle button,
    /// drawn at the tail of `show_separator` for `outer`), and `show_separator` is still going
    /// to be called for the nodes below it further down the same loop — reading their geometry
    /// from [`Self::layout`], which up to this point describes the grouping we have just
    /// replaced. So the last thing this does is re-run the layout pass over the rewritten
    /// subtree, and the rest of the pass sees the shape that now exists.
    ///
    /// The cost of not doing it used to be worse than a misdrawn frame: `show_separator` pushed
    /// `fraction` into a band derived from whatever rectangle it read, on every frame, drag or
    /// no drag — so a stale rectangle shorter than `2 * separator.extra` (whose band is the
    /// single point 0.5) *overwrote* the ratio the transposition had just computed. That is how
    /// transposing after dragging the outer divider used to snap one of the two new inner
    /// dividers back to dead centre. Only a gesture writes `fraction` now (see `SeparatorBand`),
    /// so what is left here is one frame of dividers painted and hit-tested against a shape that
    /// no longer exists — pinned by
    /// `the_toggle_leaves_the_geometry_map_describing_the_tree_it_just_wrote`, which has to read
    /// the map *inside* the editing frame, since any quiet frame rebuilds it.
    fn transpose_cross_split(
        &mut self,
        pixels_per_point: f32,
        crossings: &Crossings,
        index: usize,
    ) {
        let Crossings {
            outer,
            outer_horizontal,
            bands,
            outer_bounds,
            ..
        } = crossings;
        let [band0, band1] = bands;
        let [i, j] = crossings.at[index].0;

        // The crossing line, along the bands' axis. Averaged: the two dividers are allowed to
        // differ by up to `TOLERANCE`, that being the point of the tolerance.
        let line = 0.5 * (band0.bounds[i + 1] + band1.bounds[j + 1]);
        let (span_start, span_end) = band0.span();
        let cross_fraction = (line - span_start) / (span_end - span_start);
        let stack_fraction =
            (outer_bounds[1] - outer_bounds[0]) / (outer_bounds[2] - outer_bounds[0]);

        // The pool of split ids the new shape is built out of: exactly the two chains being
        // taken apart. `outer` is not in it — it stays where it is, because its own parent
        // points at it — and the arithmetic leaves none over: `(n - 1) + (m - 1)` ids in, two
        // halves plus `(k - 1) + (n - k - 1) + (l - 1) + (m - l - 1)` chain splits out.
        let mut pool = band0.dividers.iter().chain(&band1.dividers).copied();
        let inner_horizontal = !outer_horizontal;

        let near = rebuild_half(
            bands,
            [0, 0],
            [i, j],
            *outer_horizontal,
            stack_fraction,
            &mut pool,
        );
        let far = rebuild_half(
            bands,
            [i + 1, j + 1],
            [band0.parts.len() - 1, band1.parts.len() - 1],
            *outer_horizontal,
            stack_fraction,
            &mut pool,
        );
        assert!(
            pool.next().is_none(),
            "a transposition needs exactly as many splits as the chains it took apart"
        );

        let shape = Regroup::Split {
            id: outer.node,
            horizontal: inner_horizontal,
            fraction: cross_fraction,
            children: [Box::new(near), Box::new(far)],
        };
        // Through the tree rather than by assigning `Node`s: subtrees change parent here, and a
        // child's back-pointer and the subtree's collapsing bookkeeping live outside the `Node`
        // being assigned. See `Tree::regroup`.
        self.dock_state[outer.surface].regroup(outer.node, &shape);

        // Bring the geometry map back in step with the shape we just wrote (see the note on
        // staleness above). `max_rect` is the surface root's rectangle — the same value
        // `render_nodes` hands to `compute_rect_sizes`, recorded by `allocate_area_for_root_node`.
        // Parents before children: each call writes its children's rectangles, which the calls
        // after it cut their own children out of.
        let root = self.dock_state[outer.surface]
            .root()
            .expect("the surface being laid out has a root: `outer` lives in it");
        let max_rect = self
            .layout
            .rect(NodePath::new(outer.surface, root))
            .expect("the root was laid out at the top of this pass");
        let mut queue = VecDeque::from([outer.node]);
        while let Some(node) = queue.pop_front() {
            let Some(children) = self.dock_state[outer.surface].children(node) else {
                continue;
            };
            self.compute_rect_sizes(
                pixels_per_point,
                NodePath::new(outer.surface, node),
                max_rect,
            );
            queue.extend(children);
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{CentralPanel, Context, Id, Pos2, RawInput, Ui, Vec2, WidgetText};
    use proptest::prelude::*;

    use super::*;
    use crate::geom::{Point, Size};
    use crate::layout::DockLayout;
    use crate::{DockState, Node, NodeId, Split, Style, SurfaceIndex, TabViewer};

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

    /// Renders the dock until its geometry has settled, leaving it in `ctx` memory exactly as
    /// `DockArea::show_inside_with_response` normally would inside a real app.
    ///
    /// Two frames, not one. This used to be load-bearing for a reason that is gone — the pass
    /// wrote `fraction` back on every frame, so the tree and the map could be one step apart
    /// after a single frame — and it is kept because egui itself settles over frames (hover
    /// state, auto-sizing) and a test that aims at geometry wants a scene that has stopped
    /// moving. See `SeparatorBand` for what stopped being true.
    fn render(ctx: &Context, state: &mut DockState<u32>, style: &Style, id: Id) {
        run_frame(ctx, state, style, id, vec![]);
        run_frame(ctx, state, style, id, vec![]);
    }

    /// Runs one real headless frame with the given input `events` fed to it.
    fn run_frame(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        events: Vec<egui::Event>,
    ) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            events,
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

    /// Drags from `from` to `to` over several frames, the way a hand would — egui only calls
    /// a press a *drag* once the pointer has travelled past a threshold.
    fn drag(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        from: Pos2,
        to: Pos2,
    ) {
        use egui::{Event, Modifiers, PointerButton};

        run_frame(ctx, state, style, id, vec![Event::PointerMoved(from)]);
        run_frame(
            ctx,
            state,
            style,
            id,
            vec![Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
        );
        for step in 1..=4u8 {
            let t = f32::from(step) / 4.0;
            run_frame(
                ctx,
                state,
                style,
                id,
                vec![Event::PointerMoved(from + (to - from) * t)],
            );
        }
        run_frame(
            ctx,
            state,
            style,
            id,
            vec![Event::PointerButton {
                pos: to,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        );
        run_frame(ctx, state, style, id, vec![Event::PointerMoved(to)]);
    }

    fn fraction_of(state: &DockState<u32>, id: NodeId) -> f32 {
        state[NodePath::new(SurfaceIndex::main(), id)]
            .get_split()
            .expect("node is a split")
            .fraction
    }

    fn leaf_rects(layout: &DockLayout, leaves: &[NodeId]) -> Vec<Rect> {
        leaves
            .iter()
            .map(|id| {
                layout
                    .rect(NodePath::new(SurfaceIndex::main(), *id))
                    .expect("leaf was laid out this frame")
            })
            .collect()
    }

    fn assert_rects_close(before: &[Rect], after: &[Rect]) {
        assert_eq!(before.len(), after.len());
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

        assert_eq!(
            toggle_centers(
                &ctx,
                &mut state,
                &Style::default(),
                id,
                NodePath::new(SurfaceIndex::main(), outer)
            )
            .len(),
            1
        );
    }

    #[test]
    fn detects_perfect_cross_vertical() {
        let (mut state, outer, _) = build_cross(false, 0.5, 0.5);
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        render(&ctx, &mut state, &Style::default(), id);

        assert_eq!(
            toggle_centers(
                &ctx,
                &mut state,
                &Style::default(),
                id,
                NodePath::new(SurfaceIndex::main(), outer)
            )
            .len(),
            1
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

        assert!(
            toggle_centers(
                &ctx,
                &mut state,
                &Style::default(),
                id,
                NodePath::new(SurfaceIndex::main(), outer)
            )
            .is_empty()
        );
    }

    /// Two stacked bands: the top one split into 2 columns, the bottom one into 3, with the
    /// top band's divider lined up with the bottom band's *left* divider. On screen that is a
    /// perfect "+": one vertical line running the full height of both bands, crossing the
    /// line between them. The toggle belongs there.
    ///
    /// It used not to be offered. The detector compared each side's *root* divider and nothing
    /// else: a three-column band is a chain, `H(H(C, D), E)` here, so its root divider is the
    /// one between `D` and `E`, and the aligned `C|D` divider one level deeper was invisible.
    /// Line the top divider up with `D|E` instead and the very same screen *was* detected —
    /// which is the shape of the report: with a 2-band over a 3-band, the crossing only ever
    /// seemed to exist between the last two.
    #[test]
    fn detects_a_cross_where_a_band_has_three_columns() {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let style = Style::default();

        let mut state = DockState::new(vec![0u32]);
        let top_left = state.main_surface().root().unwrap();

        // Root: two bands, one above the other.
        let [_, bottom_left] = state.split(
            NodePath::new(SurfaceIndex::main(), top_left),
            Split::Below,
            0.5,
            Node::leaf(1u32),
        );
        let outer = state.main_surface().root().unwrap();

        // Top band: two columns.
        let [_, top_right] = state.split(
            NodePath::new(SurfaceIndex::main(), top_left),
            Split::Right,
            0.5,
            Node::leaf(2u32),
        );

        // Bottom band: three columns, built so the band's own (root) divider is the
        // right-hand one — `H(H(C, D), E)`.
        let bottom_path = NodePath::new(SurfaceIndex::main(), bottom_left);
        state.split(bottom_path, Split::Right, 0.75, Node::leaf(3u32));
        let [_, bottom_mid] = state.split(bottom_path, Split::Right, 0.5, Node::leaf(4u32));
        let bottom_inner = state.main_surface().parent(bottom_left).unwrap();

        render(&ctx, &mut state, &style, id);

        // Put the bottom band's left divider exactly under the top band's, by measurement
        // rather than by construction: the two are cut from rectangles of different width, so
        // no pair of fractions chosen in advance lands them on the same pixel.
        let rect = |ctx: &Context, n: NodeId| {
            DockLayout::load(ctx, id)
                .rect(NodePath::new(SurfaceIndex::main(), n))
                .expect("laid out this frame")
        };
        let target_x = 0.5 * (rect(&ctx, top_left).right() + rect(&ctx, top_right).left());
        let inner_rect = rect(&ctx, bottom_inner);
        state[NodePath::new(SurfaceIndex::main(), bottom_inner)]
            .get_split_mut()
            .expect("the bottom band's inner node is a split")
            .fraction = (target_x - inner_rect.min.x) / inner_rect.width();
        render(&ctx, &mut state, &style, id);

        // The "+" is really on screen. Without this the assertion below could pass on a scene
        // that has no crossing to find in the first place.
        let bottom_divider_x =
            0.5 * (rect(&ctx, bottom_left).right() + rect(&ctx, bottom_mid).left());
        let top_divider_x = 0.5 * (rect(&ctx, top_left).right() + rect(&ctx, top_right).left());
        assert!(
            (bottom_divider_x - top_divider_x).abs() <= Crossings::TOLERANCE,
            "the scene was not built as intended: the two dividers are at {top_divider_x} and \
             {bottom_divider_x}, so there is no crossing for the detector to miss"
        );

        assert_eq!(
            toggle_centers(
                &ctx,
                &mut state,
                &style,
                id,
                NodePath::new(SurfaceIndex::main(), outer)
            )
            .len(),
            1,
            "a crossing that is on screen was not detected"
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

        // Both children are plain leaves here, not splits at all: two bands of one part each,
        // no dividers, nothing to cross.
        assert!(
            toggle_centers(
                &ctx,
                &mut state,
                &Style::default(),
                id,
                NodePath::new(SurfaceIndex::main(), outer)
            )
            .is_empty()
        );
    }

    /// Reported bug: with the cross toggle button present, dragging one inner separator
    /// (say `c0`'s, between `a`/`b`) was observed to also drag the *other* inner separator
    /// (`c1`'s, between `c`/`d`) — the two moved together, and hover flickered.
    #[test]
    fn dragging_one_inner_separator_does_not_move_the_other() {
        let (mut state, _outer, [a, b, c, _d]) = build_cross(true, 0.5, 0.5);
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let style = Style::default();

        render(&ctx, &mut state, &style, id);
        let layout = DockLayout::load(&ctx, id);
        let a_rect = layout.rect(NodePath::new(SurfaceIndex::main(), a)).unwrap();
        let b_rect = layout.rect(NodePath::new(SurfaceIndex::main(), b)).unwrap();

        // The midpoint of `c0`'s own divider (between `a` and `b`).
        let at = Pos2::new(a_rect.center().x, 0.5 * (a_rect.bottom() + b_rect.top()));
        let to = at + Vec2::new(0.0, 60.0);

        let c0 = state.main_surface().parent(a).unwrap();
        let c1 = state.main_surface().parent(c).unwrap();
        let c0_before = fraction_of(&state, c0);
        let c1_before = fraction_of(&state, c1);

        drag(&ctx, &mut state, &style, id, at, to);

        let c0_after = fraction_of(&state, c0);
        let c1_after = fraction_of(&state, c1);

        assert_ne!(
            c0_before, c0_after,
            "the dragged separator itself did not move"
        );
        assert_eq!(
            c1_before, c1_after,
            "dragging c0's inner separator must not move c1's"
        );
    }

    /// Reported bug: with tabs open in a floating window that holds a cross, the window crept
    /// upwards frame after frame, for as long as it was on screen and untouched.
    ///
    /// The button used to be drawn through [`Ui::scope_builder`], which ends by calling
    /// `advance_cursor_after_rect` on the **parent** `Ui` — the one the whole dock is drawn
    /// into. So every toggle on screen pushed that `Ui`'s cursor a spacing past the bottom of
    /// the dock and grew its `min_rect` with it. On the main surface that is invisible: nothing
    /// is allocated after the dock, and the panel's size does not come from its content. Inside
    /// an [`egui::Window`] the content's `min_rect` *is* the size the window asks for, so the
    /// window grew by a spacing every frame — and `constrain_to(bounds)`, which keeps it on
    /// screen, turned growth at the bottom edge into a slide at the top.
    ///
    /// A window is therefore the only scene that can see this, and the oracle is the weakest
    /// one there is: a scene nobody touches does not move.
    #[test]
    fn a_cross_in_a_window_does_not_make_the_window_creep() {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        // The window is 400 px tall, so its two rows are well under the default 175 px margin
        // and `parts_can_be_renested` would decline — with no button there is no bug to see.
        let style = band_style();

        let mut state = DockState::new(vec![0u32]);
        let window = state.add_window(vec![1u32]);
        state
            .get_window_state_mut(window)
            .expect("the window was just added")
            .set_position(Point::new(120.0, 120.0))
            .set_size(Size::new(600.0, 400.0));

        // A 2x2 cross inside the window.
        let top_left = state[window].root().unwrap();
        let [_, top_right] = state.split(
            NodePath::new(window, top_left),
            Split::Right,
            0.5,
            Node::leaf(2u32),
        );
        let outer = NodePath::new(window, state[window].root().unwrap());
        state.split(
            NodePath::new(window, top_left),
            Split::Below,
            0.5,
            Node::leaf(3u32),
        );
        state.split(
            NodePath::new(window, top_right),
            Split::Below,
            0.5,
            Node::leaf(4u32),
        );

        // Let the window settle at the size it was asked for: `set_size` is one-shot, and the
        // frames after it are the ones that auto-size from the content.
        for _ in 0..4 {
            run_frame(&ctx, &mut state, &style, id, vec![]);
        }

        assert_eq!(
            toggle_centers(&ctx, &mut state, &style, id, outer).len(),
            1,
            "the window holds no cross, so there is no button whose drawing could move it"
        );

        let window_rect = |ctx: &Context| {
            DockLayout::load(ctx, id)
                .rect(outer)
                .expect("the window surface was laid out this frame")
        };
        let settled = window_rect(&ctx);
        for _ in 0..10 {
            run_frame(&ctx, &mut state, &style, id, vec![]);
        }
        let later = window_rect(&ctx);

        assert!(
            (settled.min - later.min).length() < 0.5 && (settled.max - later.max).length() < 0.5,
            "ten quiet frames moved the window from {settled:?} to {later:?}"
        );
    }

    /// Clicks once at `at`, over several frames (press + release + settle).
    fn click(ctx: &Context, state: &mut DockState<u32>, style: &Style, id: Id, at: Pos2) {
        use egui::{Event, Modifiers, PointerButton};

        run_frame(ctx, state, style, id, vec![Event::PointerMoved(at)]);
        run_frame(
            ctx,
            state,
            style,
            id,
            vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
        );
        run_frame(
            ctx,
            state,
            style,
            id,
            vec![Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        );
        run_frame(ctx, state, style, id, vec![]);
    }

    /// Rests the pointer at `at` for a few frames, without pressing anything.
    ///
    /// More than one frame on purpose: egui decides what is hovered at the *end* of a frame,
    /// from the widget rectangles registered during it, so a widget learns it is hovered one
    /// frame after the pointer arrives.
    fn hover(ctx: &Context, state: &mut DockState<u32>, style: &Style, id: Id, at: Pos2) {
        for _ in 0..3 {
            run_frame(ctx, state, style, id, vec![egui::Event::PointerMoved(at)]);
        }
    }

    /// Clicks at `at` and reports whether the grouping flipped — that is, whether the press
    /// reached the toggle rather than the separator underneath it.
    fn click_flips(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        outer: NodePath,
        at: Pos2,
    ) -> bool {
        let was_horizontal = state[outer].is_horizontal();
        click(ctx, state, style, id, at);
        state[outer].is_horizontal() != was_horizontal
    }

    /// A fresh 2x2 cross, rendered, with the crossing's position measured off the screen.
    ///
    /// Its own [`Context`], and that is the point: whether the button is holding the pointer
    /// lives in egui memory, and two scenes built the same way have the same node ids and so
    /// the same button id. Sharing a context between two presses would let the first one leave
    /// the button armed for the second — which is exactly the difference the tests below are
    /// trying to measure.
    fn cross_scene(style: &Style, id: Id) -> (Context, DockState<u32>, NodePath, Pos2) {
        let ctx = Context::default();
        let (mut state, outer_id, _) = build_cross(true, 0.5, 0.5);
        let outer = NodePath::new(SurfaceIndex::main(), outer_id);
        render(&ctx, &mut state, style, id);
        let center =
            toggle_center(&ctx, &mut state, style, id, outer).expect("the toggle is there");
        (ctx, state, outer, center)
    }

    /// The magnet: a press that misses the drawn square by less than `catch_extra` is still the
    /// button, not the separator underneath it.
    ///
    /// A miss here is not a no-op — the press lands on the separator and starts a resize — so
    /// the second half of the test is what keeps the first honest: just outside the catch zone,
    /// with the button cold, the press must *not* toggle. Otherwise "the click toggled" would
    /// also be true of a button that had quietly swallowed the whole line.
    #[test]
    fn the_toggle_catches_a_press_that_misses_the_drawn_square() {
        let id = Id::new(DOCK_ID);
        let style = band_style();
        let toggle = &style.cross_split_toggle;

        // Along the outer divider, so a press that is not caught lands squarely on a separator.
        let near = Vec2::new(0.0, toggle.size * 0.5 + toggle.catch_extra - 1.0);
        let far = Vec2::new(
            0.0,
            toggle.size * 0.5 + toggle.catch_extra + toggle.hold_extra - 1.0,
        );

        let (ctx, mut state, outer, center) = cross_scene(&style, id);
        assert!(
            click_flips(&ctx, &mut state, &style, id, outer, center + near),
            "a press {}px off the crossing, inside the catch zone, did not reach the toggle",
            near.y
        );

        let (ctx, mut state, outer, center) = cross_scene(&style, id);
        assert!(
            !click_flips(&ctx, &mut state, &style, id, outer, center + far),
            "a press {}px off the crossing reached a button whose catch zone ends at {}px",
            far.y,
            toggle.size * 0.5 + toggle.catch_extra
        );
    }

    /// The hysteresis: the zone that *keeps* the pointer is wider than the zone that caught it.
    ///
    /// One point, pressed twice: once with the pointer arriving cold, once after it has rested
    /// on the button. Only the second toggles. A single radius cannot tell those two apart, and
    /// that is the whole point — sitting at the edge of one radius, a pixel of jitter changes
    /// the cursor, the highlight, and what a click will do.
    ///
    /// The cold half of this is the same press the magnet test uses as its control, on purpose:
    /// what is a miss when you arrive is a hit when you were already there.
    #[test]
    fn a_button_holding_the_pointer_keeps_it_past_the_catch_zone() {
        let id = Id::new(DOCK_ID);
        let style = band_style();
        let toggle = &style.cross_split_toggle;

        let out_of_catch = Vec2::new(
            0.0,
            toggle.size * 0.5 + toggle.catch_extra + toggle.hold_extra - 1.0,
        );

        let (ctx, mut state, outer, center) = cross_scene(&style, id);
        assert!(
            !click_flips(&ctx, &mut state, &style, id, outer, center + out_of_catch),
            "the point this test is about is inside the *catch* zone, so it proves nothing"
        );

        let (ctx, mut state, outer, center) = cross_scene(&style, id);
        hover(&ctx, &mut state, &style, id, center);
        assert!(
            click_flips(&ctx, &mut state, &style, id, outer, center + out_of_catch),
            "the button let the pointer go {}px out, inside its {}px hold zone",
            out_of_catch.y,
            toggle.size * 0.5 + toggle.catch_extra + toggle.hold_extra
        );
    }

    /// Where the toggle buttons on `outer`'s line currently sit, in screen order along it.
    /// Empty if no crossing is detected and no button is on screen at all.
    fn toggle_centers(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        outer: NodePath,
    ) -> Vec<Pos2> {
        let mut area = DockArea::new(state).id(id);
        area.layout = DockLayout::load(ctx, id);
        area.detect_crossings(outer, style.separator.extra)
            .map(|crossings| crossings.at.iter().map(|&(_, center)| center).collect())
            .unwrap_or_default()
    }

    /// Where the *only* toggle button on `outer`'s line sits. Panics if there is more than one:
    /// a helper that silently picked the first would let a test aimed at one crossing pass on a
    /// scene that grew a second.
    fn toggle_center(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        outer: NodePath,
    ) -> Option<Pos2> {
        let centers = toggle_centers(ctx, state, style, id, outer);
        assert!(centers.len() <= 1, "more than one crossing: {centers:?}");
        centers.first().copied()
    }

    /// Presses the toggle button through a real headless click and asserts that it was there
    /// to be pressed and that it did flip the grouping — a click that lands on nothing would
    /// otherwise let every "nothing moved" assertion below pass vacuously.
    fn press_toggle(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        outer: NodePath,
    ) {
        let center =
            toggle_center(ctx, state, style, id, outer).expect("the toggle button is on screen");
        press_toggle_at(ctx, state, style, id, outer, center);
    }

    /// [`press_toggle`] for a line that carries more than one button: which of them to press is
    /// the caller's business, everything that has to be true afterwards is not.
    fn press_toggle_at(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        outer: NodePath,
        center: Pos2,
    ) {
        let was_horizontal = state[outer].is_horizontal();
        click(ctx, state, style, id, center);
        assert_eq!(
            state[outer].is_horizontal(),
            !was_horizontal,
            "clicking the toggle did not flip the grouping"
        );
        // The rectangles are only half of what a regrouping has to get right: two of the four
        // grandchildren change parent, and a `Node` carries neither the child's back-pointer to
        // its parent nor the subtree's collapsing bookkeeping. A tree that draws correctly and
        // is wired wrong stays quiet until something walks *up* from a moved node — a later
        // split or leaf removal — and panics there instead, one gesture removed from the cause.
        assert_eq!(
            state.validate(),
            Ok(()),
            "the toggle left the tree structurally invalid"
        );
    }

    /// Reported bug: toggle a cross to rows (one full-width divider), drag that divider down,
    /// toggle back to columns — and only one of the two restored column dividers followed the
    /// drag; the other snapped back to where it sat *before* the drag.
    ///
    /// Root cause was mid-pass staleness: `transpose_cross_split` rewrites the tree from
    /// inside `show_separator` for the outer node, and the same loop then ran `show_separator`
    /// for the two edited children against `layout` rectangles still describing the previous
    /// grouping. One of those stale rectangles (the old bottom row, 321 px) was shorter than
    /// `2 * separator.extra`, which collapses that separator's clamp interval to the single
    /// point 0.5 — so the fraction the transposition had just written was overwritten with
    /// "dead centre", which is exactly where the divider had been before the drag.
    ///
    /// The oracle is the feature's whole promise: a toggle moves no pixel.
    ///
    /// Two things closed that root cause, and this test now only sees the second one:
    /// `show_separator` no longer writes `fraction` outside a gesture (see `SeparatorBand`), and
    /// the relayout keeps the map in step. Removing the relayout alone leaves this test green —
    /// the staleness is real but no longer *persists* — which is why it has a test of its own,
    /// `the_toggle_leaves_the_geometry_map_describing_the_tree_it_just_wrote`.
    #[test]
    fn toggle_after_dragging_the_outer_divider_keeps_every_leaf_in_place() {
        // Start as two columns, each with its own inner divider ("0 сплит горизонтальный").
        let (mut state, outer_id, leaves) = build_cross(true, 0.5, 0.5);
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let style = Style::default();
        let outer = NodePath::new(SurfaceIndex::main(), outer_id);

        render(&ctx, &mut state, &style, id);

        // Toggle to rows: the two inner dividers become one full-width divider.
        press_toggle(&ctx, &mut state, &style, id, outer);

        // Drag that divider down, grabbing it on its left half.
        let layout = DockLayout::load(&ctx, id);
        let [c0, c1] = state.main_surface().children(outer_id).unwrap();
        let c0_rect = layout
            .rect(NodePath::new(SurfaceIndex::main(), c0))
            .unwrap();
        let c1_rect = layout
            .rect(NodePath::new(SurfaceIndex::main(), c1))
            .unwrap();
        let at = Pos2::new(
            c0_rect.min.x + c0_rect.width() * 0.25,
            0.5 * (c0_rect.bottom() + c1_rect.top()),
        );
        drag(&ctx, &mut state, &style, id, at, at + Vec2::new(0.0, 120.0));

        let dragged = leaf_rects(&DockLayout::load(&ctx, id), &leaves);
        assert!(
            (dragged[0].height() - dragged[1].height()).abs() > 100.0,
            "the drag did not actually move the divider, so the toggle below proves nothing"
        );

        // Toggle back to columns. Both column dividers must stay on the dragged line.
        press_toggle(&ctx, &mut state, &style, id, outer);
        assert_rects_close(&dragged, &leaf_rects(&DockLayout::load(&ctx, id), &leaves));
    }

    /// The transposition runs in the *middle* of a pass, so the geometry map it leaves behind
    /// must already describe the tree it just wrote — the rest of that pass reads it.
    ///
    /// This is the property the relayout at the end of `transpose_cross_split` exists for, and
    /// it needs a test of its own now. It used to be covered as a side effect: `show_separator`
    /// clamped `fraction` into a band derived from whatever rectangle it read, on every frame,
    /// so a stale rectangle did not merely draw in the wrong place — it *wrote back*, and the
    /// damage was still there on the next frame for
    /// `toggle_after_dragging_the_outer_divider_keeps_every_leaf_in_place` to see. Only a
    /// gesture writes `fraction` now, which is right and which also means that test no longer
    /// fails when the relayout is removed. What is left is a frame drawn against a shape that
    /// does not exist — dividers painted and hit-tested in the wrong place, for one frame — and
    /// that is only visible *inside* the frame that did the edit.
    ///
    /// Read on the three edited nodes rather than on the leaves on purpose: the whole promise of
    /// a transposition is that the four leaf rectangles are unchanged, so leaves cannot tell the
    /// two groupings apart. The splits can — `c0` and `c1` are the two columns before and the two
    /// rows after.
    ///
    /// The click is driven here rather than through `press_toggle`, and that is the whole test:
    /// `click` ends with a quiet frame, which recomputes the map from the tree and washes the
    /// staleness away before anything can look at it. The map has to be read on the frame that
    /// did the edit.
    #[test]
    fn the_toggle_leaves_the_geometry_map_describing_the_tree_it_just_wrote() {
        use egui::{Event, Modifiers, PointerButton};

        let (mut state, outer_id, _) = build_cross(true, 0.4, 0.6);
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let style = Style::default();
        let outer = NodePath::new(SurfaceIndex::main(), outer_id);

        render(&ctx, &mut state, &style, id);
        let center =
            toggle_center(&ctx, &mut state, &style, id, outer).expect("the toggle is on screen");
        let button = |pressed| {
            vec![Event::PointerButton {
                pos: center,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]
        };
        run_frame(
            &ctx,
            &mut state,
            &style,
            id,
            vec![Event::PointerMoved(center)],
        );
        run_frame(&ctx, &mut state, &style, id, button(true));
        // The release frame is the one that transposes, mid-pass. No quiet frame after it.
        run_frame(&ctx, &mut state, &style, id, button(false));
        assert!(
            state[outer].is_vertical(),
            "the click did not flip the grouping, so there is no mid-pass edit to judge"
        );

        let [c0, c1] = state.main_surface().children(outer_id).unwrap();
        let edited = [
            outer,
            NodePath::new(SurfaceIndex::main(), c0),
            NodePath::new(SurfaceIndex::main(), c1),
        ];
        let read = |layout: &DockLayout| {
            edited.map(|path| layout.rect(path).expect("an edited node was laid out"))
        };

        let straight_after = read(&DockLayout::load(&ctx, id));
        // A quiet frame recomputes the whole map from the tree, so this is what the edited
        // nodes' rectangles are *supposed* to be.
        render(&ctx, &mut state, &style, id);
        let settled = read(&DockLayout::load(&ctx, id));

        for ((path, stale), fresh) in edited.iter().zip(straight_after).zip(settled) {
            assert!(
                (stale.min - fresh.min).length() < 0.1 && (stale.max - fresh.max).length() < 0.1,
                "the frame that transposed the cross left {path:?} recorded as {stale:?}, while \
                 the shape it wrote puts it at {fresh:?} — the rest of that pass reads this map"
            );
        }
    }

    // ------------------------------------------------------------------------
    // n x m: two bands of arbitrary length
    //
    // Everything below is built through `build_bands`, which aims a band's dividers at
    // *absolute* positions and can nest the chain either way round. That is what makes "the
    // same picture, built two different ways" a scene a test can hold — and that scene is the
    // whole oracle for the detector, because it is the one thing a rule read off the tree
    // cannot get right and a rule read off the band cannot get wrong.
    // ------------------------------------------------------------------------

    /// The style the band scenes are built with: the default one, with the separator margin
    /// turned down.
    ///
    /// [`SeparatorStyle::extra`](crate::SeparatorStyle::extra) defaults to 175 px, which is a
    /// lot of screen — five parts across a 900 px axis simply cannot each keep it, so a scene
    /// built with the default margin would silently be a *different* scene, its dividers sitting
    /// where the clamp allowed rather than where the test asked. The margin is a style knob and
    /// the law under test says nothing about it; what it does interact with is
    /// [`Band::parts_can_be_renested`], which has its own test at the default value.
    fn band_style() -> Style {
        let mut style = Style::default();
        style.separator.extra = 4.0;
        style
    }

    /// How a band's chain of splits is nested. Both draw the same picture.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Leaning {
        /// `H(H(a, b), c)` — the band's own root divider is the *last* one.
        Left,
        /// `H(a, H(b, c))` — the band's own root divider is the *first* one.
        Right,
    }

    /// How many parts of the band `node`'s subtree contributes, i.e. how far along the band the
    /// chain below it reaches. A node of the other orientation (or a leaf) is one part, however
    /// deep it is.
    fn chain_parts(state: &DockState<u32>, node: NodeId, horizontal: bool) -> usize {
        let path = NodePath::new(SurfaceIndex::main(), node);
        let in_chain = if horizontal {
            state[path].is_horizontal()
        } else {
            state[path].is_vertical()
        };
        if !in_chain {
            return 1;
        }
        let [first, second] = state.main_surface().children(node).unwrap();
        chain_parts(state, first, horizontal) + chain_parts(state, second, horizontal)
    }

    /// Puts every divider of the band rooted at `root` on `targets` — absolute coordinates along
    /// the band's axis, ascending.
    ///
    /// Measured and repeated, rather than computed once. A split's fraction is applied to the
    /// rectangle the *renderer* hands it, and that rectangle is its parent's cut snapped to
    /// whole pixels, so a fraction worked out against the ideal interval lands up to half a
    /// pixel off — and down a chain those halves compound into a divider a pixel or more from
    /// where it was asked to be. Each sweep places the topmost divider against the rectangle it
    /// actually got and leaves a smaller error below it, so a few sweeps settle the whole chain.
    /// The check at the end is the point of the fixed budget: a scene that did not settle says
    /// so, instead of quietly being a different scene than the test believes.
    fn aim_band(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        root: NodeId,
        horizontal: bool,
        targets: &[f32],
    ) {
        for _ in 0..6 {
            let layout = DockLayout::load(ctx, id);
            aim_sweep(state, &layout, root, horizontal, targets);
            render(ctx, state, style, id);
        }

        let layout = DockLayout::load(ctx, id);
        let parts = band_parts_of(state, root, horizontal);
        let rect = |n: NodeId| {
            layout
                .rect(NodePath::new(SurfaceIndex::main(), n))
                .expect("laid out this frame")
        };
        for (k, target) in targets.iter().enumerate() {
            let got = 0.5
                * (edge(rect(parts[k]), horizontal, true)
                    + edge(rect(parts[k + 1]), horizontal, false));
            assert!(
                (got - target).abs() <= 0.51,
                "divider {k} settled at {got}, not at the requested {target}"
            );
        }
    }

    /// One sweep of [`aim_band`], parents before children.
    ///
    /// A second, independent statement of what [`Band::bounds`] means: a split's boundary sits
    /// at `fraction` of the interval it was handed, and its children are handed that interval
    /// cut at the boundary. Whether the chain leans left or right does not enter into it, which
    /// is exactly the property the scenes built on top of this are for.
    fn aim_sweep(
        state: &mut DockState<u32>,
        layout: &DockLayout,
        node: NodeId,
        horizontal: bool,
        targets: &[f32],
    ) {
        let path = NodePath::new(SurfaceIndex::main(), node);
        let in_chain = if horizontal {
            state[path].is_horizontal()
        } else {
            state[path].is_vertical()
        };
        if !in_chain {
            assert!(targets.is_empty(), "a part has no dividers to aim");
            return;
        }
        let rect = layout.rect(path).expect("laid out this frame");
        let (lo, hi) = (edge(rect, horizontal, false), edge(rect, horizontal, true));
        let [first, second] = state.main_surface().children(node).unwrap();
        // Everything the first child contributes lies before this split's own boundary, so the
        // number of parts down there names which target is this split's.
        let k = chain_parts(state, first, horizontal);
        state[path]
            .get_split_mut()
            .expect("a chain node is a split")
            .fraction = (targets[k - 1] - lo) / (hi - lo);
        aim_sweep(state, layout, first, horizontal, &targets[..k - 1]);
        aim_sweep(state, layout, second, horizontal, &targets[k..]);
    }

    /// Cuts the leaf `band` into `cuts.len() + 1` parts along `horizontal`'s axis, nested per
    /// `leaning`. Only the shape is built here; `aim_chain` places the boundaries afterwards.
    fn cut_band(
        state: &mut DockState<u32>,
        band: NodeId,
        horizontal: bool,
        parts: usize,
        leaning: Leaning,
        next_tab: &mut u32,
    ) {
        // Splitting the same *edge* part again and again is what builds a lopsided chain: away
        // from the rest of the band for `Left`, into it for `Right`. Both only ever split a
        // leaf, so no split node is ever handed to `DockState::split`.
        let split = match (horizontal, leaning) {
            (true, Leaning::Left) => Split::Left,
            (true, Leaning::Right) => Split::Right,
            (false, Leaning::Left) => Split::Above,
            (false, Leaning::Right) => Split::Below,
        };
        let mut edge = band;
        for _ in 1..parts {
            *next_tab += 1;
            let [_, fresh] = state.split(
                NodePath::new(SurfaceIndex::main(), edge),
                split,
                0.5,
                Node::leaf(*next_tab),
            );
            edge = fresh;
        }
    }

    /// Two bands filling the screen, cut at exactly the requested places.
    ///
    /// `outer_horizontal` says how the two bands sit relative to each other (side by side, or
    /// stacked); each band is then cut the other way. `cuts[b]` are the divider positions of
    /// band `b` as fractions of the band's own extent, ascending and strictly inside `0..1`;
    /// `leaning[b]` picks the nesting.
    ///
    /// Returns the state, `outer`'s id, and every leaf, so a caller can pin the picture.
    fn build_bands(
        ctx: &Context,
        style: &Style,
        id: Id,
        outer_horizontal: bool,
        cuts: [&[f32]; 2],
        leaning: [Leaning; 2],
    ) -> (DockState<u32>, NodeId, Vec<NodeId>) {
        let mut state = DockState::new(vec![0u32]);
        let first = state.main_surface().root().unwrap();
        let mut next_tab = 0u32;

        next_tab += 1;
        let [_, second] = state.split(
            NodePath::new(SurfaceIndex::main(), first),
            if outer_horizontal {
                Split::Right
            } else {
                Split::Below
            },
            0.5,
            Node::leaf(next_tab),
        );
        let outer = state.main_surface().root().unwrap();

        let inner_horizontal = !outer_horizontal;
        for (band, (cut, lean)) in [first, second].iter().zip(cuts.iter().zip(leaning)) {
            cut_band(
                &mut state,
                *band,
                inner_horizontal,
                cut.len() + 1,
                lean,
                &mut next_tab,
            );
        }
        // Read *after* cutting: splitting a leaf puts a fresh split node in its place, so the
        // ids the bands were built from are now parts of them, not their roots.
        let roots = state.main_surface().children(outer).unwrap();

        // The chains exist but sit wherever `0.5` put them. One render gives the bands the
        // extent their parts are cut from, and the requested fractions become absolute
        // coordinates — the *same* coordinates for both bands, which is what makes "these two
        // dividers are the same line" a fact about the scene rather than a coincidence of
        // rounding.
        render(ctx, &mut state, style, id);
        let layout = DockLayout::load(ctx, id);
        let band_rect = |root: NodeId| {
            layout
                .rect(NodePath::new(SurfaceIndex::main(), root))
                .expect("laid out this frame")
        };
        let (lo, hi) = (
            edge(band_rect(roots[0]), inner_horizontal, false),
            edge(band_rect(roots[0]), inner_horizontal, true),
        );
        for (root, cut) in roots.iter().zip(cuts) {
            let targets: Vec<f32> = cut.iter().map(|f| lo + f * (hi - lo)).collect();
            aim_band(
                ctx,
                &mut state,
                style,
                id,
                *root,
                inner_horizontal,
                &targets,
            );
        }

        let leaves = state
            .main_surface()
            .breadth_first()
            .into_iter()
            .filter(|id| state[NodePath::new(SurfaceIndex::main(), *id)].is_leaf())
            .collect();
        (state, outer, leaves)
    }

    /// The parts of the band rooted at `root`, in screen order. The test's own flattening.
    fn band_parts_of(state: &DockState<u32>, root: NodeId, horizontal: bool) -> Vec<NodeId> {
        let path = NodePath::new(SurfaceIndex::main(), root);
        let in_chain = if horizontal {
            state[path].is_horizontal()
        } else {
            state[path].is_vertical()
        };
        if !in_chain {
            return vec![root];
        }
        let [first, second] = state.main_surface().children(root).unwrap();
        let mut parts = band_parts_of(state, first, horizontal);
        parts.extend(band_parts_of(state, second, horizontal));
        parts
    }

    /// The bug, stated as the class it belongs to: a screen has the crossings it has, and how
    /// the tree happens to nest the chains that produce them is not allowed to change that.
    ///
    /// Four nestings of one picture — a 3-part band over a 4-part band, sharing two divider
    /// positions — must offer the very same two buttons at the very same two points. Before the
    /// band model, `Left`/`Left` and `Right`/`Right` disagreed with each other: each nesting
    /// pushes a different divider to the root of its chain, and the root pair was all the
    /// detector ever compared.
    #[test]
    fn the_nesting_of_a_band_does_not_change_which_crossings_exist() {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let style = band_style();

        // Two shared positions (0.3 and 0.7) and one that only the lower band has.
        let upper: &[f32] = &[0.3, 0.7];
        let lower: &[f32] = &[0.3, 0.55, 0.7];

        let mut seen: Vec<(Leaning, Leaning, Vec<Pos2>)> = Vec::new();
        for first in [Leaning::Left, Leaning::Right] {
            for second in [Leaning::Left, Leaning::Right] {
                let (mut state, outer, _) =
                    build_bands(&ctx, &style, id, false, [upper, lower], [first, second]);
                let centers = toggle_centers(
                    &ctx,
                    &mut state,
                    &style,
                    id,
                    NodePath::new(SurfaceIndex::main(), outer),
                );
                seen.push((first, second, centers));
            }
        }

        // Coverage, not decoration: "they all agree" is also true of four empty lists, and an
        // empty list is precisely what the old detector produced for three of these four.
        assert_eq!(
            seen[0].2.len(),
            2,
            "the scene does not carry the two crossings this test is about: {:?}",
            seen[0].2
        );

        let (_, _, expected) = &seen[0];
        for (first, second, centers) in &seen[1..] {
            assert_eq!(
                centers.len(),
                expected.len(),
                "{first:?}/{second:?} offers {} buttons where {:?}/{:?} offers {}",
                centers.len(),
                seen[0].0,
                seen[0].1,
                expected.len()
            );
            for (a, b) in expected.iter().zip(centers) {
                assert!(
                    (*a - *b).length() < 1.0,
                    "{first:?}/{second:?} puts a button at {b:?}, {:?}/{:?} at {a:?} — same \
                     picture, different answer",
                    seen[0].0,
                    seen[0].1
                );
            }
        }
    }

    /// A "+" whose picture cannot be rebuilt is not offered.
    ///
    /// The separator margin is a floor on how close a boundary may come to either end of the
    /// interval it is cut from, so on an interval shorter than twice the margin the boundary is
    /// pinned to the middle — and the parts that come out are shorter than the margin itself. A
    /// transposition re-cuts those parts from *different* intervals, and a fraction that lands
    /// outside the new interval's band is drawn clamped: the picture would jump.
    ///
    /// So the same scene is built twice and the margin is the only thing that differs. With a
    /// small one the button is there; with the default 175 px it is not, and the second half is
    /// what this test is for. The first half is what keeps it honest — without it, "no button"
    /// would also be the answer for a scene that never had a crossing.
    #[test]
    fn a_cross_whose_parts_are_thinner_than_the_margin_is_not_offered() {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);

        // A cross squeezed into a short strip: `SCREEN` is 900 tall, the strip a fifth of it,
        // so each band's vertical extent is far below `2 * extra` and both inner dividers are
        // pinned to the middle of it.
        let build = |style: &Style| {
            let mut state = DockState::new(vec![0u32]);
            let strip = state.main_surface().root().unwrap();
            state.split(
                NodePath::new(SurfaceIndex::main(), strip),
                Split::Below,
                0.2,
                Node::leaf(1u32),
            );
            let [_, right] = state.split(
                NodePath::new(SurfaceIndex::main(), strip),
                Split::Right,
                0.5,
                Node::leaf(2u32),
            );
            let outer = state.main_surface().parent(strip).unwrap();
            state.split(
                NodePath::new(SurfaceIndex::main(), strip),
                Split::Below,
                0.5,
                Node::leaf(3u32),
            );
            state.split(
                NodePath::new(SurfaceIndex::main(), right),
                Split::Below,
                0.5,
                Node::leaf(4u32),
            );
            render(&ctx, &mut state, style, id);
            (state, NodePath::new(SurfaceIndex::main(), outer))
        };

        let lenient = band_style();
        let (mut state, outer) = build(&lenient);
        assert_eq!(
            toggle_centers(&ctx, &mut state, &lenient, id, outer).len(),
            1,
            "the scene has no crossing at all, so the margin cannot be what removes it"
        );

        let default = Style::default();
        let (mut state, outer) = build(&default);
        assert!(
            toggle_centers(&ctx, &mut state, &default, id, outer).is_empty(),
            "a crossing was offered whose parts are too thin to be re-cut without moving"
        );
    }

    /// The reported scene, carried through: a 2-part band over a 3-part band, pressed at the
    /// crossing that only the flattened model can see. Not a pixel may move, the tree must stay
    /// valid, and pressing the same point again must bring the original grouping back.
    #[test]
    fn transposing_a_two_over_three_cross_moves_nothing() {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let style = band_style();

        let (mut state, outer_id, leaves) = build_bands(
            &ctx,
            &style,
            id,
            false,
            [&[0.4], &[0.4, 0.75]],
            // Left-leaning, so the shared divider (0.4) is *not* the lower band's root one.
            [Leaning::Left, Leaning::Left],
        );
        let outer = NodePath::new(SurfaceIndex::main(), outer_id);

        let before = leaf_rects(&DockLayout::load(&ctx, id), &leaves);
        let center =
            toggle_center(&ctx, &mut state, &style, id, outer).expect("the toggle is on screen");

        press_toggle_at(&ctx, &mut state, &style, id, outer, center);
        assert_rects_close(&before, &leaf_rects(&DockLayout::load(&ctx, id), &leaves));

        // The crossing is still the same point on screen — it is now `outer`'s own divider
        // meeting the two halves' — so pressing there again is the round trip.
        press_toggle_at(&ctx, &mut state, &style, id, outer, center);
        assert_rects_close(&before, &leaf_rects(&DockLayout::load(&ctx, id), &leaves));
        assert!(
            state[outer].is_vertical(),
            "the round trip did not restore the original grouping"
        );
    }

    /// One line, two "+"s. Each has its own button, and pressing either transposes around *that*
    /// one — the other crossing is not what gets pivoted on.
    #[test]
    fn a_line_with_two_crossings_offers_a_button_at_each() {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let style = band_style();

        let (mut state, outer_id, leaves) = build_bands(
            &ctx,
            &style,
            id,
            false,
            [&[0.25, 0.6], &[0.25, 0.45, 0.6]],
            [Leaning::Left, Leaning::Right],
        );
        let outer = NodePath::new(SurfaceIndex::main(), outer_id);

        let before = leaf_rects(&DockLayout::load(&ctx, id), &leaves);
        let centers = toggle_centers(&ctx, &mut state, &style, id, outer);
        assert_eq!(centers.len(), 2, "expected a button at each shared divider");
        assert!(
            centers[0].x < centers[1].x,
            "crossings come out in screen order: {centers:?}"
        );

        // Press the second one. The picture must not move, and the tree must stay valid — the
        // half that keeps three parts on one side and two on the other is where a rebuild that
        // miscounts the chain shows up.
        press_toggle_at(&ctx, &mut state, &style, id, outer, centers[1]);
        assert_rects_close(&before, &leaf_rects(&DockLayout::load(&ctx, id), &leaves));
    }

    /// The five places a band in the property test below may be cut at, as fractions of its
    /// extent. Spaced far apart on purpose: two dividers a pixel from each other would be one
    /// line as far as [`Crossings::TOLERANCE`] is concerned, and a scene that cannot say how
    /// many crossings it has cannot be an oracle for how many were found.
    const GRID: [f32; 5] = [0.15, 0.3, 0.45, 0.6, 0.75];

    /// The positions `mask` selects out of [`GRID`], ascending.
    fn cuts_of(mask: u8) -> Vec<f32> {
        GRID.iter()
            .enumerate()
            .filter(|(k, _)| mask & (1 << k) != 0)
            .map(|(_, at)| *at)
            .collect()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// The law, on bands of any length and any nesting: the buttons offered on a line are
        /// the positions **both** bands are cut at, and pressing one of them moves nothing.
        ///
        /// The count is checked against the masks the scene was built from — a number that
        /// knows nothing about trees, bands, or which divider ended up at the root of a chain.
        /// That independence is the whole reason the test exists: a detector that read the tree
        /// instead of the picture agreed with it only when the shared divider happened to be
        /// the root one on both sides.
        #[test]
        fn the_crossings_on_a_line_are_the_cuts_both_bands_share(
            outer_horizontal in any::<bool>(),
            first_mask in 1u8..32,
            second_mask in 1u8..32,
            first_left in any::<bool>(),
            second_left in any::<bool>(),
            which in 0usize..8,
        ) {
            let shared = (first_mask & second_mask).count_ones() as usize;
            prop_assume!(shared > 0);

            let ctx = Context::default();
            let id = Id::new(DOCK_ID);
            let style = band_style();
            let leaning = |left| if left { Leaning::Left } else { Leaning::Right };

            let (first_cuts, second_cuts) = (cuts_of(first_mask), cuts_of(second_mask));
            let (mut state, outer_id, leaves) = build_bands(
                &ctx, &style, id,
                outer_horizontal,
                [&first_cuts, &second_cuts],
                [leaning(first_left), leaning(second_left)],
            );
            let outer = NodePath::new(SurfaceIndex::main(), outer_id);

            let centers = toggle_centers(&ctx, &mut state, &style, id, outer);
            prop_assert_eq!(
                centers.len(),
                shared,
                "bands cut at {:?} and {:?} share {} positions but offer {} buttons",
                first_cuts, second_cuts, shared, centers.len()
            );

            // Pressing one of them: not a pixel moves, and pressing the same point again — it
            // is still a crossing, now between `outer`'s own divider and the two halves' —
            // brings the original grouping back.
            let before = leaf_rects(&DockLayout::load(&ctx, id), &leaves);
            let center = centers[which % centers.len()];

            press_toggle_at(&ctx, &mut state, &style, id, outer, center);
            assert_rects_close(&before, &leaf_rects(&DockLayout::load(&ctx, id), &leaves));

            press_toggle_at(&ctx, &mut state, &style, id, outer, center);
            assert_rects_close(&before, &leaf_rects(&DockLayout::load(&ctx, id), &leaves));
            prop_assert_eq!(
                state[outer].is_horizontal(),
                outer_horizontal,
                "the round trip did not restore the original grouping"
            );
        }

        /// The whole point of the feature: toggling a cross split must not move a single
        /// pixel on screen, and toggling back must restore the original grouping.
        ///
        /// Driven through a real click on the button rather than by calling
        /// `transpose_cross_split` directly: the edit lands in the *middle* of a separator
        /// pass, and the rest of that pass runs against whatever the edit left behind (which is
        /// precisely how `toggle_after_dragging_the_outer_divider_keeps_every_leaf_in_place`
        /// used to fail). Calling the surgery on a bare `DockArea` and re-rendering afterwards
        /// skips the only frame where that can happen, so it can only prove the arithmetic.
        #[test]
        fn transpose_preserves_leaf_rects_and_round_trips(
            outer_horizontal in any::<bool>(),
            outer_fraction in 0.05f32..0.95,
            inner_fraction in 0.05f32..0.95,
        ) {
            let (mut state, outer_id, leaves) = build_cross(outer_horizontal, outer_fraction, inner_fraction);
            let ctx = Context::default();
            let id = Id::new(DOCK_ID);
            let style = band_style();
            let outer = NodePath::new(SurfaceIndex::main(), outer_id);

            render(&ctx, &mut state, &style, id);
            let before = leaf_rects(&DockLayout::load(&ctx, id), &leaves);

            press_toggle(&ctx, &mut state, &style, id, outer);
            let after = leaf_rects(&DockLayout::load(&ctx, id), &leaves);
            assert_rects_close(&before, &after);

            prop_assert_eq!(
                state[outer].is_horizontal(),
                !outer_horizontal,
                "orientation did not flip"
            );

            // Toggling back must restore both the original geometry and the original grouping.
            press_toggle(&ctx, &mut state, &style, id, outer);
            let round_tripped = leaf_rects(&DockLayout::load(&ctx, id), &leaves);
            assert_rects_close(&before, &round_tripped);

            prop_assert_eq!(state[outer].is_horizontal(), outer_horizontal, "orientation not restored");
        }
    }
}
