//! Where separators meet: the handle drawn there, the drag that moves all of them at once, and
//! the transposition offered where four panels meet.
//!
//! A split's two children are divided by a line running their full extent. Every divider *of*
//! those children ends on that line, and where it does, separators meet:
//!
//! * a **tee** — three panels, two separators: the line that runs through, and the one that
//!   stops on it;
//! * a **cross** — four panels, three separators: the line, and a divider on each side of it
//!   that are one and the same line on screen.
//!
//! Both get a small square handle **under the pointer and only there** — there is one at every
//! junction of every line, and painted cold they are a grid of squares laid over the panels for
//! nobody.
//!
//! **Both are dragged, and the drag moves every separator that meets there at once** — the panels
//! around the point are resized in both directions by one gesture, which could only be done in two
//! or three drags before. At a tee that is the line plus the divider ending on it; at a crossing it
//! is the line plus the divider on *each* side of it, moved together so they stay one line.
//!
//! A crossing was not dragged at all until 2026-08-10, on the argument that two dividers aligned to
//! within [`CrossSplitToggleStyle::align_tolerance`] are a coincidence rather than a corner, so
//! resizing four panels off one is a gesture nobody asked for. Overruled from the screen («в целом
//! её таскать можно»), and what the argument leaves behind is the *shape* being part of what the
//! hand holds: a crossing dragged as a crossing keeps moving both dividers for the whole gesture,
//! and a tee that happens to line up with a neighbour halfway through is still a tee. That is
//! [`crate::JunctionArms`], recorded at `drag_started`.
//!
//! A cross is offered one thing on top of the drag. It is ambiguous — the exact same rectangles are
//! produced by either grouping (rows of columns, or columns of rows) and nothing on screen says
//! which one the tree holds — so **ctrl+clicking** it swaps the two representations without moving
//! a single pixel. The modifier is what tells that click from the short press a drag comes out as.
//!
//! A handle is on screen only under the pointer, whatever its shape: a handle takes the point away
//! from the separators under it (egui drops the layers behind a widget covering the pointer), so one
//! at every junction, painted cold, would be a grid of squares that also made every line hard to
//! grab — see `draw_one_handle`.
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
//! off it in one line: **a junction is a position either neighbouring band has a divider at, and
//! a crossing is one they both do**. Neither `n` nor `m` appears in it, and neither does depth.

use std::collections::VecDeque;

use egui::{
    CursorIcon, Id, Order, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
    epaint::CornerRadius, pos2, vec2,
};

use crate::core::tree::regroup::Regroup;
use crate::dock_area::events::DockEvent;
use crate::dock_area::state::{DragSubject, JunctionArms, State};
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

/// Which separators meet at a junction — and with them, how many panels it separates.
///
/// Two kinds of one thing: both are dragged by the same code, through the same clamp. What the
/// kind decides is what is drawn on the handle and whether a ctrl+click has anything to do, and
/// both of those follow from the shape rather than being attached to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JunctionKind {
    /// Four panels. Both bands are divided here and the two dividers are one line on screen;
    /// `[i, j]` name which divider of each band, as an index into that band's own list.
    Cross([usize; 2]),

    /// Three panels. Only the band `side` is divided here, at its divider `divider`; the line
    /// between `outer`'s two children runs through, and this one ends on it.
    Tee { side: usize, divider: usize },
}

impl JunctionKind {
    /// The dividers this junction is made of, as `(band, divider index in that band)`.
    ///
    /// The one place the two kinds differ in arity, and everything that has to visit "every
    /// divider of this junction" — the room the handle may take, the ids it is keyed by, the
    /// boundaries a drag moves — goes through it rather than matching on the kind again.
    fn dividers(self) -> impl Iterator<Item = (usize, usize)> {
        match self {
            Self::Cross([i, j]) => [Some((0, i)), Some((1, j))],
            Self::Tee { side, divider } => [Some((side, divider)), None],
        }
        .into_iter()
        .flatten()
    }
}

/// One point where separators meet, on the line between a split's two children.
#[derive(Clone, Copy, Debug)]
struct Junction {
    kind: JunctionKind,

    /// Where it is on screen.
    center: Pos2,
}

/// Every junction on the line between one split's two children, together with the two bands they
/// were found in — which the drag and the transposition both need, so they are kept rather than
/// re-derived.
struct Junctions {
    outer: NodePath,

    /// Orientation of `outer` itself: `true` if [`crate::Node::Horizontal`].
    outer_horizontal: bool,

    /// The two bands, in `outer`'s child order. Both run *across* `outer`'s own axis: a divider
    /// that reaches the line between the children has to.
    bands: [Band; 2],

    /// `outer`'s own three boundaries along its own axis: the far edge of the first band, the
    /// line between the two, and the far edge of the second.
    outer_bounds: [f32; 3],

    /// Whether a transposition may be offered on the crossings of this line at all.
    ///
    /// A fact about that one gesture, and no longer about whether the junctions exist. A
    /// transposition rebuilds both chains and can only promise "no pixel moves" if both can be
    /// re-nested (see [`Band::parts_can_be_renested`]); a *drag* is meaningful either way, so
    /// suppressing the whole detection here — which is what this used to do — would have taken
    /// the handle away from layouts that have every use for it.
    can_transpose: bool,

    /// The junctions, in screen order along the line.
    at: Vec<Junction>,
}

impl Junctions {
    /// The floor under [`CrossSplitToggleStyle::align_tolerance`]: **one device pixel**.
    ///
    /// A floor, not the policy — the policy is the style's, in points, and it is normally the
    /// wider of the two. What this pins down is what the *smallest* useful answer means. Every
    /// boundary in a [`Band`] is the midpoint of two edges [`DockArea::compute_rect_sizes`] has
    /// already snapped to whole device pixels, so two dividers aimed at one line come out at
    /// most one device pixel apart. A tolerance below that would refuse pairs that are drawn on
    /// the same pixel — "the same line" would mean "the same float", which is not a thing a
    /// layout can promise and not a thing an eye can check.
    const TOLERANCE_FLOOR_DEVICE_PIXELS: f32 = 1.0;

    /// How far apart two dividers may sit and still be one crossing, in points: the style's
    /// `align_tolerance`, never less than one device pixel.
    ///
    /// The floor is expressed in device pixels and not in points, which is the whole reason it
    /// is a function of `pixels_per_point` rather than a constant: a flat `1.0` is one device
    /// pixel on the ppp-1 screen it was picked on and two of them at ppp 2, so the same crate
    /// was twice as willing to call a jog a cross on a high-density display.
    ///
    /// The slack is float arithmetic, not policy: a bound is a sum of two snapped edges halved,
    /// so a gap that *is* the tolerance can land a few ulps the wrong side of the limit.
    fn tolerance(toggle: &CrossSplitToggleStyle, pixels_per_point: f32) -> f32 {
        toggle
            .align_tolerance
            .max(Self::TOLERANCE_FLOOR_DEVICE_PIXELS / pixels_per_point)
            + 1e-3
    }

    /// The widest square the handle at junction `index` may occupy.
    ///
    /// The handle sits on two or three dividers at once and answers to presses over its whole
    /// reach, so every point it covers is a point that can no longer be grabbed to drag one of
    /// them on its own. That is the trade the magnet buys, and it is a good one only while the
    /// handle stays small next to what it sits on. The bound is the distance to the nearest
    /// thing that has to stay reachable: across the line, the far edges of the two bands
    /// (`outer`'s own two children); along it, the ends of the two parts each of the junction's
    /// dividers separates.
    ///
    /// Nothing in the default style comes near it — `separator.extra` keeps every part at least
    /// 175 px long, against a 38 px handle at its widest. It binds where the style is loosened
    /// or the window squeezed, and there the answer is a handle that shrinks with the layout
    /// rather than one that swallows it.
    fn room_at(&self, index: usize) -> f32 {
        // Across the line: `outer`'s two children, measured along `outer`'s own axis.
        let mut room = (self.outer_bounds[1] - self.outer_bounds[0])
            .min(self.outer_bounds[2] - self.outer_bounds[1]);
        // Along the line: the two parts each of this junction's dividers separates. A tee has
        // one such divider and a cross has two, which is the only difference.
        for (band, k) in self.at[index].kind.dividers() {
            let band = &self.bands[band];
            room = room
                .min(band.bounds[k + 1] - band.bounds[k])
                .min(band.bounds[k + 2] - band.bounds[k + 1]);
        }
        room
    }
}

/// The tier a handle's own layer belongs in, given the tier the dock itself is drawn in.
///
/// A handle has to win the pointer against the separators it sits on — that is what a layer of its
/// own is *for* — and it has to lose to egui's menus and popups, which are areas in
/// [`Order::Foreground`]. One tier above the dock's own content says both, and says the second one
/// only where a tier is left to be above: a dock hosted in a floating window already draws in
/// [`Order::Middle`], and egui has nothing between that and `Foreground`.
///
/// Why the tier alone used to be the wrong fix for the paint, not merely the press: egui ranks
/// layers for the pointer by `Areas::compare_order`, where a layer that is not an [`egui::Area`]
/// compares below every area of the same tier, because `None < Some(i)`. It paints them by
/// `GraphicLayers::drain`, which walks a tier's areas in that same order and then sweeps up every
/// layer of the tier it has not seen yet. So a non-area layer is ranked *under* the areas of its
/// tier and painted *over* them: at `Foreground` a handle that was a bare layer lost the press to
/// a menu, as it should, and still put a square on top of it. `draw_one_handle` now draws the
/// handle as a real [`egui::Area`] instead, which is what registers a layer into that order and
/// so paints and ranks it the same way a menu does. The tier computed here is still what decides
/// *which* areas the handle has to rank correctly against — one tier above the dock's own content,
/// capped at `Foreground` because there is nothing above that for a window-hosted dock to give it.
fn handle_layer(dock: Order) -> Order {
    match dock {
        Order::Background => Order::Middle,
        // A window surface, or anything else an application hosts the dock in: already at or above
        // `Middle`, where the only tier left is the one menus live in.
        _ => Order::Foreground,
    }
}

/// How far `point` is from `rect` in the metric a *square* reaches in: the larger of the two
/// axis gaps, and zero if the point is inside.
///
/// The button is a square centred on the crossing, so a square of half-side `h` covers `rect`
/// exactly when **both** gaps are under `h` — which makes the larger of the two the distance
/// that matters. Measuring in a straight line instead would call a divider that is far away
/// along one axis and level along the other "close", and shrink the button for nothing.
fn square_gap(rect: Rect, point: Pos2) -> f32 {
    let x = (rect.min.x - point.x).max(point.x - rect.max.x).max(0.0);
    let y = (rect.min.y - point.y).max(point.y - rect.max.y).max(0.0);
    x.max(y)
}

/// What the pointer asked of a junction handle this frame.
///
/// The handle answers rather than acts, because acting rewrites the geometry every handle on the
/// line was read off — including the ones not drawn yet.
#[derive(Clone, Copy, Debug)]
enum Grip {
    /// Nothing, which is also what a plain click amounts to.
    Idle,

    /// Held and moved this much since the last frame: every separator meeting here follows.
    Resize(Vec2),

    /// Ctrl+clicked on a crossing: swap the grouping around it.
    Transpose,
}

/// The arms of a handle's icon, as unit directions.
///
/// The cross keeps the four diagonals it has always had: the pinwheel reads as "swap the
/// grouping", which is what a ctrl+click there still does. A tee has no transposition to offer,
/// and gets three arms drawn along the separators that actually meet at it — two for the line
/// that runs through, one for the one that stops. So the icon says which junction this is, and
/// says it out of the geometry rather than out of a name: count the arms and you have counted
/// the panels.
fn icon_arms(kind: JunctionKind, outer_horizontal: bool) -> Vec<Vec2> {
    match kind {
        JunctionKind::Cross(_) => [45.0_f32, 135.0, 225.0, 315.0]
            .into_iter()
            .map(|degrees| {
                let angle = degrees.to_radians();
                vec2(angle.cos(), angle.sin())
            })
            .collect(),
        JunctionKind::Tee { side, .. } => {
            // The line between `outer`'s children runs across `outer`'s own axis; the stem is
            // the divider that ends on it, reaching back into the band it belongs to — the
            // first band lies on the near side of the line, the second on the far side.
            let (through, stem) = if outer_horizontal {
                ([vec2(0.0, -1.0), vec2(0.0, 1.0)], vec2(-1.0, 0.0))
            } else {
                ([vec2(-1.0, 0.0), vec2(1.0, 0.0)], vec2(0.0, -1.0))
            };
            let stem = if side == 0 { stem } else { -stem };
            vec![through[0], through[1], stem]
        }
    }
}

/// One arrow of a handle's icon: a stem from near the centre outwards, and two barbs at its tip.
fn draw_arrow(painter: &Painter, center: Pos2, dir: Vec2, arm: f32, stroke: Stroke) {
    let tip = center + dir * arm;
    let base = center + dir * (arm * 0.3);
    painter.line_segment([base, tip], stroke);
    let perp = vec2(-dir.y, dir.x) * (arm * 0.25);
    painter.line_segment([tip, tip - dir * (arm * 0.35) + perp], stroke);
    painter.line_segment([tip, tip - dir * (arm * 0.35) - perp], stroke);
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
    /// produce a NaN fraction — which this crate has already been bitten by once (see the project's
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

    /// Every junction on the line between `outer`'s two children, in screen order.
    ///
    /// `outer` must already be known to be a split (parent) node; callers of `show_separator`
    /// establish that before this runs.
    ///
    /// How far out of line two dividers may be and still be one crossing rather than two tees
    /// comes from `toggle`; `pixels_per_point` is what puts the floor under it in the points
    /// this geometry is measured in — see [`Junctions::tolerance`].
    fn detect_junctions(
        &self,
        outer: NodePath,
        extra: f32,
        toggle: &CrossSplitToggleStyle,
        pixels_per_point: f32,
    ) -> Option<Junctions> {
        let outer_horizontal = self.dock_state[outer].is_horizontal();
        let [c0, c1] = self.child_paths(outer);

        // A divider that can reach the line between the two children runs *across* `outer`'s
        // own axis, so it comes from a split of the opposite orientation — which is the
        // orientation each band is flattened along.
        let inner_horizontal = !outer_horizontal;
        let band0 = self.band(c0, inner_horizontal)?;
        let band1 = self.band(c1, inner_horizontal)?;
        let can_transpose =
            band0.parts_can_be_renested(extra) && band1.parts_can_be_renested(extra);

        let (c0_rect, c1_rect) = (self.layout.rect(c0)?, self.layout.rect(c1)?);
        let outer_bounds = [
            edge(c0_rect, outer_horizontal, false),
            0.5 * (edge(c0_rect, outer_horizontal, true) + edge(c1_rect, outer_horizontal, false)),
            edge(c1_rect, outer_horizontal, true),
        ];
        let at_line = |line: f32| {
            if outer_horizontal {
                pos2(outer_bounds[1], line)
            } else {
                pos2(line, outer_bounds[1])
            }
        };

        // Both lists ascend, so one merge walk consumes them: it finds every pair that is the
        // same line and, unlike a nested scan, cannot hand one divider two partners — which
        // would put two handles on one point the moment two dividers ever sat a pixel apart.
        // Whatever it does not pair is not skipped but emitted as a tee: a divider that has no
        // partner across the line still *ends* on it, and that is a junction of two separators.
        // Walking both lists to exhaustion, rather than stopping at the shorter one, is what
        // makes that true of the tail as well as of the middle.
        let (first, second) = (band0.divider_positions(), band1.divider_positions());
        let tolerance = Junctions::tolerance(toggle, pixels_per_point);
        let (mut i, mut j) = (0, 0);
        let mut at = Vec::new();
        while i < first.len() || j < second.len() {
            let pair = first.get(i).zip(second.get(j));
            if let Some((&a, &b)) = pair
                && (a - b).abs() <= tolerance
            {
                at.push(Junction {
                    kind: JunctionKind::Cross([i, j]),
                    center: at_line(0.5 * (a + b)),
                });
                i += 1;
                j += 1;
                continue;
            }
            // Screen order: whichever of the two heads comes first along the line is the next
            // junction, and the list that ran out has no head at all.
            let take_first = pair.is_none_or(|(a, b)| a < b) && i < first.len();
            if take_first {
                at.push(Junction {
                    kind: JunctionKind::Tee {
                        side: 0,
                        divider: i,
                    },
                    center: at_line(first[i]),
                });
                i += 1;
            } else {
                at.push(Junction {
                    kind: JunctionKind::Tee {
                        side: 1,
                        divider: j,
                    },
                    center: at_line(second[j]),
                });
                j += 1;
            }
        }

        Some(Junctions {
            outer,
            outer_horizontal,
            bands: [band0, band1],
            outer_bounds,
            can_transpose,
            at,
        })
    }

    /// The room the handle at junction `index` may take: what the two bands leave it, bounded
    /// once more by every *other* divider actually drawn in this surface.
    ///
    /// [`Junctions::room_at`] reads the bands, and a band is a list of parts — each of which is a
    /// whole subtree, opaque to it. A part two columns wide may carry a divider of its own a
    /// level down, sitting a few pixels from the junction, and a bound that only knows "the
    /// nearest boundary in *this* band" cheerfully lets the handle cover it. Covering it is not
    /// cosmetic: the handle answers to presses over its whole reach and sits in a
    /// [`Order::Foreground`] layer, so every point it takes is a point where that divider can no
    /// longer be grabbed on its own.
    ///
    /// The honest bound is the distance to the nearest divider on screen, and the layout pass
    /// knows exactly where those are — `separator_rect` is the same derivation the dividers are
    /// drawn from, so this cannot answer about a line that is not there. The dividers the
    /// junction is *made of* are skipped: the handle is meant to sit on them, and they run
    /// through its centre.
    ///
    /// Same convention as `room_at`: `room` is the handle's full width and is bounded by a
    /// one-sided distance, so at the limit the handle reaches half way to what it must not
    /// cover. Nothing in the default style comes near either bound — `separator.extra` keeps
    /// every part 175 px long, against a 38 px handle at its widest.
    fn handle_room(
        &self,
        junctions: &Junctions,
        index: usize,
        separator: &SeparatorStyle,
        pixels_per_point: f32,
    ) -> f32 {
        let Junction { kind, center } = junctions.at[index];
        let surface = junctions.outer.surface;
        let own: Vec<NodeId> = std::iter::once(junctions.outer.node)
            .chain(
                kind.dividers()
                    .map(|(band, k)| junctions.bands[band].dividers[k]),
            )
            .collect();

        let mut room = junctions.room_at(index);
        for node in self.dock_state[surface].breadth_first() {
            if own.contains(&node) {
                continue;
            }
            let path = NodePath::new(surface, node);
            let Some(divider) = self.separator_rect(path, separator, pixels_per_point) else {
                continue;
            };
            room = room.min(square_gap(divider, center));
        }
        room
    }

    /// Draws a handle at every junction on `outer`'s line and carries out whatever the pointer
    /// asked of one of them: a drag resizes, a ctrl+click on a crossing transposes.
    ///
    /// Each handle is drawn in a layer of its own, one tier above the surrounding separators'
    /// (see [`handle_layer`]). Layer order is how egui resolves overlapping widgets — a widget in
    /// a higher-order layer wins hover/click over anything underneath regardless of screen
    /// position or `interact()` call order — so this is what stops the resize cursor and the
    /// separator's own hover highlight from "bubbling" through the handle on top of it, with no
    /// need to carve a hole out of the separator's interact rect.
    pub(super) fn draw_junction_handles(
        &mut self,
        ui: &mut Ui,
        outer: NodePath,
        style: &SeparatorStyle,
        toggle: &CrossSplitToggleStyle,
        state: &mut State,
    ) {
        if !self.show_junction_handles {
            return;
        }
        let pixels_per_point = ui.ctx().pixels_per_point();
        let pass = ui.ctx().cumulative_pass_nr();

        // **A gesture keeps the junction it grabbed, and keeps it against the detector.** The
        // handles below are read off this frame's geometry, and a junction drag is *moving* that
        // geometry — so the answer can change under the hand: two dividers the drag brings into
        // line stop being two tees and become one crossing, which is a different `kind`, a
        // different key, and so a different widget id. The handle that held the gesture is then
        // not drawn at all, egui goes on dragging a widget nobody registers, and the resize simply
        // stops until the hand opens. Reported from the screen ("когда тройник пытается стать
        // крестовиной там что-то происходит"), and the rule Стас gave for it is the one written
        // here: what the hand holds is decided once, at `drag_started`, and every frame after that
        // follows *that* subject.
        //
        // So a live junction gesture takes this function over: one handle, named by the id the
        // gesture was begun under, placed where its own two boundaries cross *now*. The detector
        // is not consulted, which also means none of the reasons a junction may stop being offered
        // — a neighbouring divider drawn too close for the button (see `draw_one_handle`'s room
        // gate), a crossing that has no transposition to give — can take a gesture away
        // mid-flight. Measured: without this, the room gate alone reddens the sweep at seed 35
        // with "a live gesture was forgotten, not one whose subject died under it".
        if let Some(drag) = state.in_flight_at(pass)
            && let DragSubject::Junction {
                outer: held,
                outer_horizontal,
                arms,
            } = drag.subject
        {
            // Another line's gesture: this line stands its handles down entirely. Same statement
            // the per-handle `dragged_elsewhere` guard used to make, one level up, where it is
            // also the answer to "who owns this pass".
            if held != outer {
                return;
            }
            let widget = drag.widget;
            if let Grip::Resize(delta) = self.follow_held_junction(
                ui,
                widget,
                held,
                outer_horizontal,
                arms,
                style,
                toggle,
                state,
            ) && self.drag_junction(
                pixels_per_point,
                held,
                outer_horizontal,
                arms,
                style.extra,
                delta,
            ) {
                state.mark_drag_moved();
                self.events.push(DockEvent::SeparatorDragging);
            }
            return;
        }

        let Some(junctions) = self.detect_junctions(outer, style.extra, toggle, pixels_per_point)
        else {
            return;
        };

        // Every handle is drawn, but at most one may be acted on: both gestures move the
        // geometry these junctions were read off, so the rest of them stop describing anything
        // the moment the first one fires. Only one handle can hold the pointer, so this is
        // bookkeeping rather than a policy — what it buys is that the loop below draws every
        // handle against one consistent picture.
        let mut acted = None;
        for index in 0..junctions.at.len() {
            match self.draw_one_handle(ui, &junctions, index, style, toggle, state) {
                Grip::Idle => {}
                grip => acted = Some((index, grip)),
            }
        }

        match acted {
            // The frame a drag *starts* on: `begin_drag` has just run inside the handle, and the
            // travel of that first frame is applied through the same path every later frame takes.
            // From the next frame on the branch at the top of this function owns the gesture.
            Some((_, Grip::Resize(delta))) => {
                let Some(&DragSubject::Junction {
                    outer,
                    outer_horizontal,
                    arms,
                }) = state.in_flight().map(|drag| &drag.subject)
                else {
                    return;
                };
                if self.drag_junction(
                    pixels_per_point,
                    outer,
                    outer_horizontal,
                    arms,
                    style.extra,
                    delta,
                ) {
                    state.mark_drag_moved();
                    self.events.push(DockEvent::SeparatorDragging);
                }
            }
            Some((index, Grip::Transpose)) => {
                self.transpose_cross_split(pixels_per_point, &junctions, index);
                self.events.push(DockEvent::CrossSplitTransposed);
            }
            Some((_, Grip::Idle)) | None => {}
        }
    }

    /// The handle a live gesture is holding, drawn and interacted with **by its subject** rather
    /// than by anything this frame's detector says.
    ///
    /// The id is the one `begin_drag` recorded, so egui goes on talking to the same widget however
    /// the geometry moves; the place is where the gesture's own two boundaries cross *now*, read
    /// off the two separator rectangles the dock draws them from — which is the same derivation
    /// `detect_junctions` uses for a centre, minus the search for which junctions exist.
    ///
    /// What it deliberately does not do is decide whether the junction is still *offered*. That
    /// question belongs to a hand that is about to grab one, and answering it again mid-gesture is
    /// exactly the bug this exists for: see the note at the top of `draw_junction_handles`.
    ///
    /// A held junction always draws a tee's icon, and that is honest rather than a simplification:
    /// [`DragSubject::Junction`] carries one divider, a crossing is never dragged at all (it senses
    /// clicks only), so every gesture that can reach this function is a tee's — outer line plus one
    /// divider ending on it.
    #[allow(clippy::too_many_arguments)]
    fn follow_held_junction(
        &mut self,
        ui: &mut Ui,
        widget: Id,
        outer: NodePath,
        outer_horizontal: bool,
        arms: JunctionArms,
        style: &SeparatorStyle,
        toggle: &CrossSplitToggleStyle,
        state: &mut State,
    ) -> Grip {
        let pass = ui.ctx().cumulative_pass_nr();
        let pixels_per_point = ui.ctx().pixels_per_point();

        // The subject can leave the tree under the hand — a leaf closed mid-gesture takes the
        // splits above it with it — and this is where that is noticed. Asked of the tree *before*
        // the geometry, because `separator_rect` indexes the node rather than looking it up and
        // panics on a path that names nothing (`no node 0.1 in this tree`, which is how the sweep
        // reported this at seed 1, step 16). Nothing is drawn and nothing is reported; the field's
        // own liveness filter drops the gesture a pass later, which is the one divergence the
        // harness checks rather than exempts.
        let divider = arms.first();
        if self.dock_state.node(outer).is_err()
            || arms
                .dividers()
                .iter()
                .any(|path| self.dock_state.node(*path).is_err())
        {
            return Grip::Idle;
        }
        let Some(outer_rect) = self.separator_rect(outer, style, pixels_per_point) else {
            return Grip::Idle;
        };
        let Some(divider_rect) = self.separator_rect(divider, style, pixels_per_point) else {
            return Grip::Idle;
        };
        // One coordinate from each line, which is what a junction *is* — and not the intersection
        // of the two rectangles, which was tried and is empty: a divider ends **on** the line
        // rather than crossing it, so the two separator rects meet edge to edge and
        // `Rect::intersect` comes out with no area at all. Measured, because the symptom was
        // subtle: the early return that followed left the detector's own path to handle every
        // second frame, and the gesture travelled exactly half as far as the hand did.
        //
        // Which axis is which is `outer`'s: the line between its two children runs across
        // `outer`'s own axis, so for a horizontal split the line is vertical and the junctions
        // along it are picked out by the divider's *y*. Same convention as `detect_junctions`'
        // `at_line`.
        let center = if outer_horizontal {
            pos2(outer_rect.center().x, divider_rect.center().y)
        } else {
            pos2(divider_rect.center().x, outer_rect.center().y)
        };

        // Which side of the line the stem points to, for the icon alone: the band the divider
        // belongs to is the one its own line sits on, measured along `outer`'s axis.
        let side = if outer_horizontal {
            usize::from(divider_rect.center().x > outer_rect.center().x)
        } else {
            usize::from(divider_rect.center().y > outer_rect.center().y)
        };

        let drawn = Rect::from_center_size(center, Vec2::splat(toggle.size));
        // The hold margin, unconditionally: a hand that is already dragging is holding the pointer
        // by definition, so there is nothing to decide between the catch zone and the wider one.
        let hit_rect = drawn.expand(toggle.catch_extra + toggle.hold_extra);
        ui.data_mut(|data| data.insert_temp(widget, pass));

        egui::Area::new(widget)
            .order(handle_layer(ui.layer_id().order))
            .fixed_pos(hit_rect.min)
            .default_size(hit_rect.size())
            .movable(false)
            .sense(Sense::hover())
            .constrain(false)
            .show(ui.ctx(), |ui| {
                ui.set_min_size(hit_rect.size());
                let response = ui
                    .interact(hit_rect, widget, Sense::click_and_drag())
                    .on_hover_and_drag_cursor(CursorIcon::Move);

                let (fill, icon) = if response.is_pointer_button_down_on() {
                    (style.color_dragged, toggle.icon_color)
                } else {
                    (style.color_hovered, toggle.icon_color)
                };
                ui.painter().rect(
                    drawn,
                    CornerRadius::from(toggle.size * 0.25),
                    fill,
                    Stroke::NONE,
                    StrokeKind::Inside,
                );
                let stroke = Stroke::new(1.5, icon);
                let held_kind = match arms {
                    JunctionArms::Tee(_) => JunctionKind::Tee { side, divider: 0 },
                    JunctionArms::Cross(_) => JunctionKind::Cross([0, 0]),
                };
                for dir in icon_arms(held_kind, outer_horizontal) {
                    draw_arrow(ui.painter(), center, dir, toggle.size * 0.28, stroke);
                }

                if response.drag_stopped() {
                    if state.end_drag(widget).is_some_and(|drag| drag.moved) {
                        self.events.push(DockEvent::LayoutCommitted);
                    }
                    return Grip::Idle;
                }
                if response.dragged() {
                    state.keep_drag_alive(widget, pass);
                    return Grip::Resize(response.drag_delta());
                }
                Grip::Idle
            })
            .inner
    }

    /// One handle. Answers what the pointer asked of it this frame; the caller does it, because
    /// doing it invalidates the junctions this was read from.
    ///
    /// # It claims no space
    ///
    /// The handle is drawn into an [`egui::Area`] of its own, and that is not a stylistic choice
    /// over folding it into the surrounding `Ui`: an `Area` floats outside the `Ui` stack
    /// entirely (see the module doc on [`egui::Area`] itself — "no parent"), so it cannot fold
    /// its size into the parent's `min_rect` and advance its cursor the way a `ui.scope_builder`
    /// would. That is what a raw child `Ui` used to do here, and what broke: the parent is the
    /// `Ui` the entire dock is drawn into, and in a floating window that `Ui`'s `min_rect` is the
    /// size the window asks for — so each handle drawn grew the window by one item spacing,
    /// every frame, and `constrain_to` turned that into a slide up the screen once it reached
    /// the bottom. See `a_cross_in_a_window_does_not_make_the_window_creep`. A handle sitting
    /// on a separator occupies a place the layout has already accounted for; it must take none
    /// of its own.
    ///
    /// # It catches the pointer from outside itself
    ///
    /// Two radii, and what they are for is in [`CrossSplitToggleStyle`]: the pointer is caught
    /// from `catch_extra` outside the drawn square, and held until it leaves a zone
    /// `hold_extra` wider than that. Which of the two applies is state — "is this handle
    /// currently holding the pointer" — so it is remembered per handle between frames.
    ///
    /// # It is drawn only under the pointer
    ///
    /// A handle sits on a place the layout already uses, and there is one at every junction of
    /// every line: painted whether or not anything is near them, they are a grid of squares the
    /// eye has to read past to see the panels. What each of them offers is a thing you do *to
    /// the corner you are pointing at*, so the corner you are pointing at is when it has anything
    /// to say. Cold, it is not merely quiet — it is not there.
    ///
    /// The widget is still registered every frame, hovered or not: that registration is what the
    /// hit test answers "is the pointer here" *from*. Only the painting is conditional.
    ///
    /// # A crossing is dragged too, and that is newer than the rest of this
    ///
    /// It was not, until 2026-08-10: a crossing is two dividers aligned by coincidence, and
    /// resizing four panels off that was refused on the grounds that nobody asked for it. What the
    /// refusal cost was a handle that existed *only while ctrl was held* — because a handle with no
    /// gesture is worse than useless. egui drops every layer behind a widget that covers the
    /// pointer's search area (`hit_test.rs`, "nothing behind this layer could ever be interacted
    /// with"), and handles live in their own layer one tier above the dock's content (see
    /// [`handle_layer`]), so a click-only handle takes the point away from the separators under it
    /// just as surely as a draggable one.
    ///
    /// Both halves are now the ordinary case: the handle senses `click_and_drag` whatever the shape,
    /// a drag at a crossing moves the line and both dividers (see `drag_junction`), and ctrl is what
    /// tells the transposing *click* from the short press a drag comes out as. The rule the old
    /// design was built around still holds and is still what keeps this honest: **a handle exists
    /// exactly while it has a gesture to offer** — which, now, is wherever the pointer is.
    fn draw_one_handle(
        &mut self,
        ui: &mut Ui,
        junctions: &Junctions,
        index: usize,
        style: &SeparatorStyle,
        toggle: &CrossSplitToggleStyle,
        state: &mut State,
    ) -> Grip {
        let Junction { kind, center } = junctions.at[index];
        let pass = ui.ctx().cumulative_pass_nr();

        // A crossing is dragged like a tee, and offers the transposition on top of it. It used to
        // exist only while ctrl was held, on the argument that resizing four panels off two
        // dividers that merely happen to be aligned is a gesture nobody asked for — and that
        // argument was overruled from the screen: «+ не появляется. мы же условились что в целом
        // её таскать можно» (Стас, 2026-08-10). So the handle is there whenever the pointer is,
        // whatever shape the junction has, and what ctrl adds at a crossing is the *click* that
        // swaps the grouping.
        //
        // What the drag then moves is both of its dividers, together, so they stay one line — see
        // `JunctionArms::Cross` and `drag_junction`. The shape is recorded at `drag_started`, which
        // is what makes "a crossing dragged as a crossing" a fact for the whole gesture rather than
        // a re-reading of geometry the gesture is moving.
        let is_cross = matches!(kind, JunctionKind::Cross(_));
        let transposable = is_cross && junctions.can_transpose;

        // While one handle is being dragged, every other one stands down — not drawn, not
        // registered, not interacted. egui would not hand two widgets one drag, but the handles
        // are read off geometry the drag is moving: a neighbour that keeps answering can inherit
        // the gesture the moment the junction under the pointer is re-detected as a different
        // one. `pass` is what keeps a drag whose junction died from holding the rest down for
        // good — see [`crate::dock_area::state::DragInFlight::pass`].
        //
        // A *handle's* drag, and not merely "something is in flight": since the separator folded
        // into the same field, the subject has to be read, not just its presence. A crossing
        // senses clicks only, so the press that offers its toggle leaves the drag to the divider
        // underneath — the two are live at once, by design, and a handle that stood down for it
        // would take its own button off the screen the moment it was pressed.
        let dragged_elsewhere = state
            .in_flight_at(pass)
            .is_some_and(|drag| matches!(drag.subject, DragSubject::Junction { .. }));

        // Keyed by the dividers that meet here rather than by their position in the band: an id
        // has to survive a neighbouring divider being dragged past, and an index does not. The
        // side a tee's divider is on is already in the id it contributes; `outer` is in there
        // because a junction is a fact about *that* line, and two of them are told apart by it.
        let key: Vec<NodeId> = std::iter::once(junctions.outer.node)
            .chain(
                kind.dividers()
                    .map(|(band, k)| junctions.bands[band].dividers[k]),
            )
            .collect();
        let handle_id = ui.id().with((key, "junction_handle"));
        let order = handle_layer(ui.layer_id().order);

        // The button is the size the style says, or it is not there at all. It used to be scaled
        // down to whatever room the junction had (`toggle_metrics`, gone with this), and a button
        // that shrinks as its neighbours close in was the wrong answer twice over: the eye reads a
        // shrinking square as a thing being manipulated rather than offered, and the hand gets a
        // catch zone whose size depends on a layout it cannot see. So `room` — the distance to the
        // nearest divider the handle must not cover, see `handle_room` — is a **gate** now, not a
        // scale: a junction with no space for the whole button offers no handle, and the
        // separators there are grabbed the ordinary way.
        let widest = toggle.widest();
        let room = self.handle_room(junctions, index, style, ui.ctx().pixels_per_point());
        if widest <= 0.0 || room < widest {
            return Grip::Idle;
        }
        let (size, catch_extra, hold_extra) = (toggle.size, toggle.catch_extra, toggle.hold_extra);
        let drawn_idle = Rect::from_center_size(center, Vec2::splat(size));
        let catch_rect = drawn_idle.expand(catch_extra);

        // Another handle has the pointer: this one is not on screen and not in the way. Checked
        // after the id is derived — "which handle is being dragged" is a question about ids —
        // and before anything is registered or painted.
        let owns_the_drag = state
            .in_flight()
            .is_some_and(|drag| drag.widget == handle_id);
        if dragged_elsewhere && !owns_the_drag {
            return Grip::Idle;
        }

        // The grip is remembered as *when* it was last held, not as a flag. A junction can stop
        // existing while it holds the pointer — the layout changes under it — and a bare `true`
        // left in memory would arm the handle the moment the same dividers meet again, widening
        // its reach for a frame at a point the pointer merely happens to be near. A pass number
        // cannot go stale: it either names the frame before this one, or it does not.
        let held_on: Option<u64> = ui.data(|data| data.get_temp(handle_id));
        let holding = held_on.is_some_and(|last| last + 1 >= pass);
        let hit_rect = if holding {
            catch_rect.expand(hold_extra)
        } else {
            catch_rect
        };

        // A real `Area`, not a bare layer sharing its `Order`: `GraphicLayers::drain` paints a
        // tier's areas in the order `Memory::areas` ranks them in, and then sweeps up every layer
        // of that tier it has not seen — so a layer that never joined that order was always
        // painted last, on top of real areas ranked above it, however the press had already
        // resolved. An `Area` is what registers a layer into that order (`Areas::set_state`);
        // nothing short of one does. See `handle_layer`.
        //
        // `fixed_pos` and `movable(false)` say the handle's place is ours to give, every frame —
        // egui never lags the *position* behind a fixed one. The `size` it hands back a frame
        // late (`AreaState::size` is written only at the end of a pass) is accepted: a handle can
        // be a frame behind its own room shrinking, the same way `holding` above is allowed to be
        // a pass behind the pointer leaving. `sense(Sense::hover())` keeps the area's own
        // built-in "move" widget from competing with the `interact` below over the same
        // rectangle — it still promotes the handle to the top of its tier on a press or on first
        // appearing (`pointer_pressed_on_area` / `!visible_last_frame`), which is all "move to
        // top" ought to mean for something that is not itself dragged.
        egui::Area::new(handle_id)
            .order(order)
            .fixed_pos(hit_rect.min)
            .default_size(hit_rect.size())
            .movable(false)
            .sense(Sense::hover())
            .constrain(false)
            .show(ui.ctx(), |ui| {
                ui.set_min_size(hit_rect.size());

                // Both shapes are dragged now, in either direction at once, and the cursor says
                // so; a crossing also answers a ctrl+click, which is a click and not a drag.
                let response = ui
                    .interact(hit_rect, handle_id, Sense::click_and_drag())
                    .on_hover_and_drag_cursor(CursorIcon::Move);

                // A press keeps its grip even if the pointer slides off, the same way any
                // button does; without it, releasing a hair outside would both cancel the click
                // and disarm.
                let holds_now = response.hovered() || response.is_pointer_button_down_on();
                ui.data_mut(|data| {
                    if holds_now {
                        data.insert_temp(handle_id, pass);
                    } else {
                        data.remove_temp::<u64>(handle_id);
                    }
                });

                // Painted only where the pointer is (see the note above), and one rectangle
                // whether it is merely under it or held. The margins widen what answers to the
                // pointer; they do not widen what is painted. Growing the square to the catch
                // zone was tried — it made the handle more than double under the cursor at the
                // default style, which reads as the thing being dragged rather than offered.
                // Colour carries the "you have it" instead.
                //
                // `dragged()` is in there for the frames a drag travels: the pointer leads the
                // handle, which follows a frame behind, so the hover it started from is not a
                // thing that keeps being true — and a handle that vanished mid-gesture would say
                // the gesture had ended.
                if holds_now || response.dragged() {
                    let handle_size = drawn_idle.width();
                    // The palette swaps rather than shifts while it is held: whichever of the
                    // two the square takes, the icon takes the other, so the arms stay legible
                    // against it. A handle is never seen cold, so `color_idle` — the separator's
                    // own resting colour — is not one of the two.
                    // The square takes the separator's palette; the arrows take their own colour,
                    // which is the panel fill by default — a cut-out rather than a second bright
                    // grey. See `CrossSplitToggleStyle::icon_color` for why the swap that used to
                    // be here produced a plain white square with no arrows on it.
                    let (fill, icon) = if response.is_pointer_button_down_on() {
                        (style.color_dragged, toggle.icon_color)
                    } else {
                        (style.color_hovered, toggle.icon_color)
                    };
                    ui.painter().rect(
                        drawn_idle,
                        CornerRadius::from(handle_size * 0.25),
                        fill,
                        Stroke::NONE,
                        StrokeKind::Inside,
                    );
                    let stroke = Stroke::new(1.5, icon);
                    for dir in icon_arms(kind, junctions.outer_horizontal) {
                        draw_arrow(ui.painter(), center, dir, handle_size * 0.28, stroke);
                    }
                }

                // What the gesture has hold of, named once and kept: the nodes, not an index
                // into a list that is rebuilt from the geometry this drag is about to move. See
                // [`DragSubject::Junction`].
                if response.drag_started() {
                    let mut dividers = kind.dividers().map(|(band, k)| {
                        NodePath::new(junctions.outer.surface, junctions.bands[band].dividers[k])
                    });
                    let first = dividers
                        .next()
                        .expect("every junction is made of at least one divider");
                    // The shape is part of what is grabbed: a crossing dragged as a crossing moves
                    // both of its dividers for the whole gesture, whatever the detector says next
                    // frame. See `JunctionArms`.
                    let arms = match dividers.next() {
                        Some(second) => JunctionArms::Cross([first, second]),
                        None => JunctionArms::Tee(first),
                    };
                    state.begin_drag(
                        handle_id,
                        DragSubject::Junction {
                            outer: junctions.outer,
                            outer_horizontal: junctions.outer_horizontal,
                            arms,
                        },
                        response
                            .interact_pointer_pos()
                            .expect("a drag that started was pressed somewhere"),
                        pass,
                    );
                }
                if response.drag_stopped() {
                    if state.end_drag(handle_id).is_some_and(|drag| drag.moved) {
                        self.events.push(DockEvent::LayoutCommitted);
                    }
                }
                if response.dragged() {
                    // Alive this frame, so a stale entry can be told from a live one.
                    state.keep_drag_alive(handle_id, pass);
                    return Grip::Resize(response.drag_delta());
                }

                // Ctrl, because a plain click is what a press aimed at the separator underneath
                // comes out as when it does not travel far enough, and a gesture aimed at a
                // line must not rewrite the tree when it falls short.
                if response.clicked() && transposable && ui.input(|i| i.modifiers.command) {
                    return Grip::Transpose;
                }

                Grip::Idle
            })
            .inner
    }

    /// Moves the two separators the drag grabbed by `delta`. Answers whether either of them
    /// actually moved.
    ///
    /// The delta is split by axis, and which component goes where is the whole geometry of the
    /// gesture: the line between `outer`'s children runs *across* `outer`'s own axis, so the
    /// component along that axis moves `outer` itself, and the other one moves the divider that
    /// ends on it. Two boundaries, at right angles, from one gesture.
    ///
    /// It reads what the hand holds ([`DragSubject::Junction`]) and not this frame's junctions,
    /// and that is the point: the gesture keeps hold of what it grabbed even as the detector's
    /// answer moves under it.
    fn drag_junction(
        &mut self,
        pixels_per_point: f32,
        outer: NodePath,
        outer_horizontal: bool,
        arms: JunctionArms,
        extra: f32,
        delta: Vec2,
    ) -> bool {
        let (along_outer, along_bands) = if outer_horizontal {
            (delta.x, delta.y)
        } else {
            (delta.y, delta.x)
        };

        // Each is clamped on its own by `nudge_split`: they are cut from intervals at right
        // angles to each other and have nothing to stay in line with. A crossing has two of them,
        // one on each side of the line, and both take the same component — which is what keeps
        // them one line on screen while the gesture runs.
        let mut moved = self.nudge_split(outer, pixels_per_point, extra, along_outer);
        for divider in arms.dividers() {
            moved |= self.nudge_split(*divider, pixels_per_point, extra, along_bands);
        }
        moved
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
    /// does not: nothing moves but the two dividers that were out of line, and pressing the
    /// button at the same point again brings the original grouping back.
    ///
    /// Those two are what the magnet is for. They become one line — their average, see the `line`
    /// below — so each moves by half of whatever gap
    /// [`CrossSplitToggleStyle::align_tolerance`] let through, and on the pair that was already
    /// aligned that is nothing at all. It is the one movement a press is allowed to make, and it
    /// is the point of pressing.
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
        junctions: &Junctions,
        index: usize,
    ) {
        let Junctions {
            outer,
            outer_horizontal,
            bands,
            outer_bounds,
            ..
        } = junctions;
        let [band0, band1] = bands;
        let JunctionKind::Cross([i, j]) = junctions.at[index].kind else {
            unreachable!("only a crossing is offered a transposition")
        };

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
    use crate::dock_area::state::DragInFlight;
    use crate::geom::{Point, Size};
    use crate::layout::DockLayout;
    use crate::{DockState, Node, NodeId, Split, Style, SurfaceIndex, TabViewer, drag_in_flight};

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

    /// Runs one real headless frame with the given input `events` fed to it, and answers what
    /// the dock reported during it.
    ///
    /// Most callers here are asking about geometry and drop the report on the floor; the ones
    /// that are not are asking whether a gesture announced itself, which is a separate question
    /// from whether it moved anything.
    fn run_frame(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        events: Vec<egui::Event>,
    ) -> Vec<DockEvent> {
        run_frame_painting(ctx, state, style, id, events).0
    }

    /// [`run_frame`], and what the frame *painted* as well as what it reported.
    ///
    /// Only the handles' own squares come back — see [`handle_squares`]. A handle that is not
    /// drawn is the whole of what "no hover, no button" means, and nothing else in the dock's
    /// state says whether it was: the widget is registered either way (that registration is what
    /// answers "is the pointer here"), so the paint list is the only place the difference shows.
    fn run_frame_painting(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        events: Vec<egui::Event>,
    ) -> (Vec<DockEvent>, Vec<Rect>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            events,
            ..Default::default()
        };
        let mut reported = Vec::new();
        let mut output = ctx.run_ui(input, |ctx| {
            CentralPanel::default().show(ctx, |ui| {
                reported = DockArea::new(state)
                    .id(id)
                    .style(style.clone())
                    .show_inside_with_response(ui, &mut Viewer)
                    .events;
            });
        });
        let painted = handle_squares(&output.shapes);
        // Headless harness, no GPU backend to hand the delta to.
        output.textures_delta.clear();
        (reported, painted)
    }

    /// One frame's handle paint, in two parts: the square (its rectangle and fill) and the colours
    /// of every stroked shape drawn *inside* it — the arrows.
    ///
    /// Separate from [`handle_squares`] because the question is different: that one asks whether a
    /// handle was drawn at all and is deliberately blind to colour, this one asks whether what was
    /// drawn can be seen. "Inside the square" is how the arrows are told from the rest of the
    /// frame — every stroke of the dock's own separators is somewhere else on screen.
    fn handle_paint(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
    ) -> (Option<(Rect, egui::Color32)>, Vec<egui::Color32>) {
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
        output.textures_delta.clear();

        let mut square: Option<(Rect, egui::Color32)> = None;
        let mut strokes = Vec::new();
        fn walk(
            shape: &egui::Shape,
            square: &mut Option<(Rect, egui::Color32)>,
            strokes: &mut Vec<(Rect, egui::Color32)>,
        ) {
            match shape {
                egui::Shape::Rect(rect) => {
                    let (w, h) = (rect.rect.width(), rect.rect.height());
                    if (w - h).abs() < 0.5 && (4.0..=64.0).contains(&w) {
                        *square = Some((rect.rect, rect.fill));
                    }
                }
                egui::Shape::LineSegment { points, stroke } => {
                    strokes.push((Rect::from_two_pos(points[0], points[1]), stroke.color));
                }
                egui::Shape::Path(path) => {
                    // A path's stroke can be a gradient (`ColorMode::UV`); the arrows are solid,
                    // and a gradient here would be something else's shape.
                    if let egui::epaint::ColorMode::Solid(color) = path.stroke.color {
                        strokes.push((path.visual_bounding_rect(), color));
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, square, strokes);
                    }
                }
                _ => {}
            }
        }
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut square, &mut strokes);
        }
        let inside: Vec<egui::Color32> = match square {
            Some((rect, _)) => strokes
                .iter()
                .filter(|(bounds, _)| rect.expand(1.0).contains_rect(*bounds))
                .map(|(_, color)| *color)
                .collect(),
            None => Vec::new(),
        };
        (square, inside)
    }

    /// Every junction handle painted in a frame, as the square each of them draws.
    ///
    /// Read off the shapes rather than off any flag, because the flag is what is being checked.
    /// A handle is the only thing in this dock that paints a *rounded square* of a few dozen
    /// points: separators are thin rectangles with square corners, tab bars and bodies are
    /// oblong, and both are told apart from a handle by shape alone rather than by position —
    /// so a handle drawn somewhere it should not be is caught as readily as one missing.
    fn handle_squares(shapes: &[egui::epaint::ClippedShape]) -> Vec<Rect> {
        fn walk(shape: &egui::Shape, out: &mut Vec<Rect>) {
            match shape {
                egui::Shape::Rect(rect) => {
                    let (w, h) = (rect.rect.width(), rect.rect.height());
                    if (w - h).abs() < 0.5 && (4.0..=64.0).contains(&w) {
                        out.push(rect.rect);
                    }
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// Drags from `from` to `to` over several frames, the way a hand would — egui only calls
    /// a press a *drag* once the pointer has travelled past a threshold. Answers everything the
    /// dock reported over the whole gesture.
    fn drag(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        from: Pos2,
        to: Pos2,
    ) -> Vec<DockEvent> {
        use egui::{Event, Modifiers, PointerButton};

        let mut reported = run_frame(ctx, state, style, id, vec![Event::PointerMoved(from)]);
        reported.extend(run_frame(
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
        ));
        for step in 1..=4u8 {
            let t = f32::from(step) / 4.0;
            reported.extend(run_frame(
                ctx,
                state,
                style,
                id,
                vec![Event::PointerMoved(from + (to - from) * t)],
            ));
        }
        reported.extend(run_frame(
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
        ));
        reported.extend(run_frame(
            ctx,
            state,
            style,
            id,
            vec![Event::PointerMoved(to)],
        ));
        reported
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
        // Measured against the *floor* — one device pixel — and not against the style's magnet,
        // which is points wide and would happily call a botched aim a crossing. This scene puts
        // the two dividers on the same pixel by measurement; a guard that accepted whatever the
        // magnet accepts would stop noticing when that aim went wrong.
        let strict = CrossSplitToggleStyle {
            align_tolerance: 0.0,
            ..CrossSplitToggleStyle::default()
        };
        assert!(
            (bottom_divider_x - top_divider_x).abs()
                <= Junctions::tolerance(&strict, ctx.pixels_per_point()),
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
        click_holding(ctx, state, style, id, at, egui::Modifiers::NONE);
    }

    /// [`click`] with ctrl held — the gesture that transposes a crossing.
    fn ctrl_click(ctx: &Context, state: &mut DockState<u32>, style: &Style, id: Id, at: Pos2) {
        click_holding(ctx, state, style, id, at, egui::Modifiers::COMMAND);
    }

    /// One click at `at` with `modifiers` held down for the whole gesture.
    ///
    /// The modifiers arrive as their own [`egui::Event::ModifiersChanged`] and are taken back
    /// afterwards, because that is the only thing egui updates `InputState::modifiers` from —
    /// what a widget reads through `ui.input(|i| i.modifiers)`. Putting them on the
    /// `PointerButton` events alone leaves that state untouched, and a ctrl+click assembled that
    /// way arrives as a plain one; releasing them afterwards keeps a test's ctrl out of the
    /// frames that follow, which live in the same [`Context`].
    ///
    /// **Two** warm-up frames, not one — this is what `draw_one_handle`'s handle-as-`Area` costs
    /// a crossing's handle the first time it is ever shown (a tee's is up from frame one, so it
    /// never pays this). egui hit-tests a frame against the *previous* frame's committed
    /// [`egui::containers::area::AreaState::interactable`], and an area whose state did not
    /// exist yet commits that as `false` for the sizing pass egui forces on it — so a press on
    /// the very frame the handle first appears is invisible twice over: once because the square
    /// itself is not painted, and once more because the frame after that still cannot be hit
    /// against. A real user pauses between pressing ctrl and clicking far longer than two
    /// frames; a test that presses on the first or second one is testing a gap nothing else
    /// reaches, not the gesture.
    fn click_holding(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        at: Pos2,
        modifiers: egui::Modifiers,
    ) {
        use egui::{Event, Modifiers, PointerButton};

        for _ in 0..2 {
            run_frame(
                ctx,
                state,
                style,
                id,
                vec![Event::ModifiersChanged(modifiers), Event::PointerMoved(at)],
            );
        }
        for pressed in [true, false] {
            run_frame(
                ctx,
                state,
                style,
                id,
                vec![Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers,
                }],
            );
        }
        run_frame(
            ctx,
            state,
            style,
            id,
            vec![Event::ModifiersChanged(Modifiers::NONE)],
        );
    }

    /// How far out of line two dividers may be and still be offered a button is the style's
    /// `align_tolerance`, in points — and it never falls below one **device** pixel, which is
    /// what `0.0` means and why that meaning survives a change of `pixels_per_point`.
    ///
    /// Both halves are here because each alone would pass on a broken rule. Only the floor, and
    /// the knob could be read as a constant; only the knob, and a `0.0` style would refuse pairs
    /// drawn on the very same pixel — "the same line" would mean "the same float". The floor's
    /// own point is density: one *point* of misalignment is one device pixel at ppp 1 and two at
    /// ppp 2, so a floor stated in points would have been twice as loose on a high-density
    /// screen (it once was — a flat `1.0`).
    ///
    /// The geometry is dictated rather than rendered. The distances at stake are smaller than
    /// the pixel snapping the layout pass applies, so a scene aimed through fractions cannot
    /// state one exactly — and a test that cannot say what gap it built cannot be an oracle for
    /// which gaps are accepted. The tree is a real 2x2 cross; only the rectangles are hand-cut,
    /// and they are cut as a partition of the screen, the way the layout pass would.
    #[test]
    fn the_magnet_reaches_as_far_as_the_style_says_and_never_less_than_a_device_pixel() {
        let (mut state, outer_id, [top_left, bottom_left, top_right, bottom_right]) =
            build_cross(true, 0.5, 0.5);
        let outer = NodePath::new(SurfaceIndex::main(), outer_id);
        let [left_half, right_half] = state
            .main_surface()
            .children(outer_id)
            .expect("the root of a cross is a split");

        // The two columns, and inside each the horizontal divider that has to meet the other.
        // `gap` is how far the right column's divider sits below the left one's.
        let scene = |gap: f32| {
            let mut layout = DockLayout::default();
            let mut put = |node: NodeId, rect: Rect| {
                layout.set_rect(NodePath::new(SurfaceIndex::main(), node), rect);
            };
            let (left, right) = (0.0..=600.0, 601.0..=1200.0);
            let half = 0.5; // half a separator: the gap the midpoint of a boundary is read from
            put(left_half, Rect::from_x_y_ranges(left.clone(), 0.0..=900.0));
            put(
                right_half,
                Rect::from_x_y_ranges(right.clone(), 0.0..=900.0),
            );
            put(
                top_left,
                Rect::from_x_y_ranges(left.clone(), 0.0..=450.0 - half),
            );
            put(
                bottom_left,
                Rect::from_x_y_ranges(left, 450.0 + half..=900.0),
            );
            put(
                top_right,
                Rect::from_x_y_ranges(right.clone(), 0.0..=450.0 + gap - half),
            );
            put(
                bottom_right,
                Rect::from_x_y_ranges(right, 450.0 + gap + half..=900.0),
            );
            layout
        };

        let mut offered = |gap: f32, pixels_per_point: f32, align_tolerance: f32| {
            let toggle = CrossSplitToggleStyle {
                align_tolerance,
                ..CrossSplitToggleStyle::default()
            };
            let mut area = DockArea::new(&mut state).id(Id::new(DOCK_ID));
            area.layout = scene(gap);
            area.detect_junctions(
                outer,
                band_style().separator.extra,
                &toggle,
                pixels_per_point,
            )
            // A *crossing*, not a junction: outside the magnet's reach the same two dividers are
            // still there, each ending on the line as a tee of its own. What the tolerance
            // decides is whether they are one line or two, and asking "is there anything here"
            // would answer yes either way.
            .is_some_and(|junctions| {
                junctions
                    .at
                    .iter()
                    .any(|junction| matches!(junction.kind, JunctionKind::Cross(_)))
            })
        };

        // The knob: what is offered is what the style asked for, in points, at any density.
        assert!(
            offered(7.0, 1.0, 8.0) && offered(7.0, 2.0, 8.0),
            "a gap inside the style's tolerance is a crossing the magnet is meant to close"
        );
        assert!(
            !offered(9.0, 1.0, 8.0) && !offered(9.0, 2.0, 8.0),
            "a gap outside the style's tolerance is a jog, and the magnet does not reach it"
        );
        assert!(
            offered(9.0, 1.0, 12.0),
            "the reach is the style's to set: the same gap a tighter style refused"
        );
        assert!(
            !offered(7.0, 1.0, 2.0),
            "a tighter style than the default has to bind, or the knob is decoration"
        );

        // The floor: `0.0` is one device pixel, and one *device* pixel at every density.
        assert!(
            offered(0.0, 1.0, 0.0) && offered(0.0, 2.0, 0.0),
            "two dividers on exactly the same line are a cross at any density"
        );
        assert!(
            offered(1.0, 1.0, 0.0),
            "one point apart is one device pixel at ppp 1 — the finest the screen can tell apart"
        );
        assert!(
            !offered(1.0, 2.0, 0.0),
            "one point apart is two device pixels at ppp 2, which is a jog, not a cross"
        );
        assert!(
            offered(0.5, 2.0, 0.0),
            "half a point is one device pixel at ppp 2, so the floor shrank with the pixel \
             rather than vanishing"
        );
        assert!(
            !offered(2.0, 1.0, 0.0),
            "the floor is one device pixel, not any pixel"
        );
    }

    /// A 2x2 cross squeezed into a strip a twentieth of the screen tall, so each of its four
    /// parts is some 20px — less than the 38px the button asks for at its widest.
    ///
    /// Its own [`Context`], for the reason [`cross_scene`] gives.
    fn squeezed_cross_scene(style: &Style, id: Id) -> (Context, DockState<u32>, NodePath, Pos2) {
        let ctx = Context::default();
        let mut state = DockState::new(vec![0u32]);
        let strip = state.main_surface().root().unwrap();
        let strip_path = NodePath::new(SurfaceIndex::main(), strip);
        state.split(strip_path, Split::Below, 0.05, Node::leaf(1u32));
        let [_, right] = state.split(strip_path, Split::Right, 0.5, Node::leaf(2u32));
        let outer = NodePath::new(
            SurfaceIndex::main(),
            state.main_surface().parent(strip).unwrap(),
        );
        state.split(strip_path, Split::Below, 0.5, Node::leaf(3u32));
        state.split(
            NodePath::new(SurfaceIndex::main(), right),
            Split::Below,
            0.5,
            Node::leaf(4u32),
        );
        render(&ctx, &mut state, style, id);
        let center = toggle_center(&ctx, &mut state, style, id, outer)
            .expect("the squeezed scene is still a cross");
        (ctx, state, outer, center)
    }

    /// The room a junction has is a **gate**, not a scale: a crossing with space for the whole
    /// button gets one, a crossing without gets none, and nothing in between gets a small one.
    ///
    /// Replaces two tests written against the old behaviour — one on `toggle_metrics`'s arithmetic
    /// (a function that no longer exists) and one asserting that a squeezed cross still answered a
    /// press at its exact centre. That second half is the assertion that flipped: a squeezed cross
    /// has no handle at all now, and a press there is a press on the separators, which is what
    /// every point of the layout that has no handle does.
    ///
    /// Both halves are here for the same reason they were before: without the roomy scene a gate
    /// that refuses everything passes, and without the squeezed one a gate that refuses nothing
    /// does.
    #[test]
    fn a_cross_without_room_for_the_whole_button_has_no_handle() {
        let id = Id::new(DOCK_ID);
        // A deliberately big button, so "the squeezed scene has no room for it" is a fact about the
        // gate and not about whatever the shipped `size` happens to be this month. The squeezed
        // scene leaves ~20pt; the default button asks for 13 and fits, which made this test pass
        // for the wrong reason the moment the defaults were trimmed.
        let mut style = band_style();
        style.cross_split_toggle.size = 40.0;
        let style = style;
        let toggle = &style.cross_split_toggle;

        let (ctx, mut state, outer, center) = cross_scene(&style, id);
        assert!(
            click_flips(&ctx, &mut state, &style, id, outer, center),
            "the roomy half of this test stopped working, so the squeezed half proves nothing"
        );
        // And it is the *whole* button there: a press as far out as the catch zone reaches still
        // lands, which is what says the size was not quietly trimmed on a roomy layout either.
        let off = Vec2::new(0.0, toggle.size * 0.5 + toggle.catch_extra - 1.0);
        let (ctx, mut state, outer, center) = cross_scene(&style, id);
        assert!(
            click_flips(&ctx, &mut state, &style, id, outer, center + off),
            "the button on a roomy cross is narrower than the style asks for: a press {}px out \
             missed it",
            off.y
        );

        // ~20px of room against a button that wants `widest()`. Nothing to press, at the centre or
        // anywhere else.
        let (ctx, mut state, outer, center) = squeezed_cross_scene(&style, id);
        assert!(
            !click_flips(&ctx, &mut state, &style, id, outer, center),
            "a cross with less room than the button needs still offered one at its centre"
        );
    }

    /// Rests the pointer at `at` for a few frames, without pressing anything.
    ///
    /// More than one frame on purpose: egui decides what is hovered at the *end* of a frame,
    /// from the widget rectangles registered during it, so a widget learns it is hovered one
    /// frame after the pointer arrives.
    fn hover(ctx: &Context, state: &mut DockState<u32>, style: &Style, id: Id, at: Pos2) {
        hover_holding(ctx, state, style, id, at, egui::Modifiers::NONE);
    }

    /// [`hover`] with `modifiers` held down throughout — the only way to rest the pointer on a
    /// crossing's handle, which is not there while ctrl is up.
    ///
    /// The modifiers are left held: this is used to set up a press that follows, and taking them
    /// back would take the handle away again between the two.
    fn hover_holding(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        at: Pos2,
        modifiers: egui::Modifiers,
    ) {
        run_frame(
            ctx,
            state,
            style,
            id,
            vec![egui::Event::ModifiersChanged(modifiers)],
        );
        for _ in 0..3 {
            run_frame(ctx, state, style, id, vec![egui::Event::PointerMoved(at)]);
        }
    }

    /// Ctrl+clicks at `at` and reports whether the grouping flipped — that is, whether the press
    /// reached the handle rather than the separator underneath it.
    fn click_flips(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        outer: NodePath,
        at: Pos2,
    ) -> bool {
        let was_horizontal = state[outer].is_horizontal();
        ctrl_click(ctx, state, style, id, at);
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

    /// A divider hidden *inside* a part still gets to be grabbed.
    ///
    /// The bands the crossing is read off see their parts as single opaque things, so a divider
    /// one level down inside a part is invisible to `Crossings::room_at` however close to the
    /// crossing it is. The button is drawn in a foreground layer and answers to presses over its
    /// whole reach, so a bound that cannot see that divider hands its grab zone away — the
    /// divider is on screen, the cursor changes over it, and pressing it toggles the grouping
    /// instead.
    ///
    /// The scene puts one 10 px from the crossing, which the default bound would have called
    /// "450 px of room".
    ///
    /// What the second half asserts changed with the sizing rule. `room` used to *scale* the
    /// button, so the scene could ask for both at once: the hidden divider free and a (smaller)
    /// button still at the crossing. `room` is a **gate** now — 10 px is less than the button's
    /// widest form, so this crossing offers no handle at all, which is the same answer by a
    /// blunter route. The control that keeps the first half honest therefore moves to a second
    /// scene: the *same* press geometry on a cross with no divider hidden near it, where the
    /// button does exist and does answer. Without that, "the press did not toggle" is satisfied by
    /// a crate whose button stopped working everywhere.
    #[test]
    fn a_divider_inside_a_part_is_not_swallowed_by_the_button() {
        /// Far enough from the crossing to be a different place, close enough that a button at
        /// its full 38 px width would cover it.
        const GAP: f32 = 10.0;

        let id = Id::new(DOCK_ID);
        let style = band_style();
        let ctx = Context::default();

        let (mut state, outer_id, [top_left, ..]) = build_cross(true, 0.5, 0.5);
        let outer = NodePath::new(SurfaceIndex::main(), outer_id);
        render(&ctx, &mut state, &style, id);
        let center =
            toggle_center(&ctx, &mut state, &style, id, outer).expect("the toggle is there");

        // Cut the quadrant above-left of the crossing in two, with the cut `GAP` short of the
        // crossing. Splitting a part does not change the band it belongs to — the new node is of
        // the other orientation, so the chain stops at it — and the crossing is still there.
        let quadrant = leaf_rects(&DockLayout::load(&ctx, id), &[top_left])[0];
        let fraction = (center.x - GAP - quadrant.min.x) / quadrant.width();
        let [_, _] = state.split(
            NodePath::new(SurfaceIndex::main(), top_left),
            Split::Right,
            fraction,
            Node::leaf(9u32),
        );
        render(&ctx, &mut state, &style, id);

        // Where it actually landed: a fraction is applied to the rectangle the renderer hands the
        // node, so aiming in points and reading back in points is the only way to know.
        let divider = leaf_rects(&DockLayout::load(&ctx, id), &[top_left])[0]
            .max
            .x;
        assert!(
            (center.x - divider - GAP).abs() < 2.0,
            "the scene was meant to put a divider {GAP}px from the crossing at {}, and it is at \
             {divider}",
            center.x
        );
        assert!(
            toggle_center(&ctx, &mut state, &style, id, outer).is_some(),
            "splitting a part removed the crossing, so this scene tests nothing"
        );

        // Press the hidden divider at its closest point to the crossing. A ctrl+click is the
        // probe because a transposition is the thing that leaves a mark on the tree; what the
        // press is really asking is which widget the point belongs to. It used also to be the
        // only probe available — the handle senses drags now, so a drag no longer falls through
        // to the separator underneath, which is the feature and is exactly why this bound has to
        // hold.
        let press = Pos2::new(divider, center.y - 5.0);
        assert!(
            !click_flips(&ctx, &mut state, &style, id, outer, press),
            "a press {GAP}px from the crossing, on a divider of its own, toggled the grouping: \
             the button is sitting on a divider it cannot see"
        );

        // And the gate did it: with 10 px of room there is no handle at the crossing either.
        assert!(
            !click_flips(&ctx, &mut state, &style, id, outer, center),
            "a crossing with {GAP}px of room still offered a button — the room gate is not wired"
        );

        // The control: the same press, on a cross whose parts carry no divider close by, does
        // reach a button. This is what says the two assertions above are about *this* layout and
        // not about a button that has stopped existing.
        let (ctx, mut state, outer, center) = cross_scene(&style, id);
        assert!(
            click_flips(&ctx, &mut state, &style, id, outer, center),
            "the roomy control lost its button, so the assertions above pass for free"
        );
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
        let style = wide_toggle_style();
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
    ///
    /// The resting half holds ctrl while it rests: a crossing's handle exists while the modifier
    /// its one gesture is named by is down, so an open-handed hover has nothing to arm.
    #[test]
    fn a_button_holding_the_pointer_keeps_it_past_the_catch_zone() {
        let id = Id::new(DOCK_ID);
        let style = wide_toggle_style();
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
        hover_holding(
            &ctx,
            &mut state,
            &style,
            id,
            center,
            egui::Modifiers::COMMAND,
        );
        assert!(
            click_flips(&ctx, &mut state, &style, id, outer, center + out_of_catch),
            "the button let the pointer go {}px out, inside its {}px hold zone",
            out_of_catch.y,
            toggle.size * 0.5 + toggle.catch_extra + toggle.hold_extra
        );
    }

    /// Every junction on `outer`'s line, in screen order along it: what kind it is, and where.
    /// Empty where the detector answers nothing at all.
    fn junctions_on(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        outer: NodePath,
    ) -> Vec<(JunctionKind, Pos2)> {
        let mut area = DockArea::new(state).id(id);
        area.layout = DockLayout::load(ctx, id);
        area.detect_junctions(
            outer,
            style.separator.extra,
            &style.cross_split_toggle,
            ctx.pixels_per_point(),
        )
        .map(|junctions| {
            junctions
                .at
                .iter()
                .map(|junction| (junction.kind, junction.center))
                .collect()
        })
        .unwrap_or_default()
    }

    /// Where the handles a ctrl+click can *transpose* sit, in screen order along `outer`'s line.
    ///
    /// Crossings only, and only where the two chains can be re-nested: that is exactly the set
    /// the toggle acts on, which is what the suite below is about. Tees carry a handle too — see
    /// [`junctions_on`] — but there is nothing to transpose at one, so counting them here would
    /// make every "how many buttons are offered" assertion mean something else.
    fn toggle_centers(
        ctx: &Context,
        state: &mut DockState<u32>,
        style: &Style,
        id: Id,
        outer: NodePath,
    ) -> Vec<Pos2> {
        let mut area = DockArea::new(state).id(id);
        area.layout = DockLayout::load(ctx, id);
        area.detect_junctions(
            outer,
            style.separator.extra,
            &style.cross_split_toggle,
            ctx.pixels_per_point(),
        )
        .filter(|junctions| junctions.can_transpose)
        .map(|junctions| {
            junctions
                .at
                .iter()
                .filter(|junction| matches!(junction.kind, JunctionKind::Cross(_)))
                .map(|junction| junction.center)
                .collect()
        })
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
        ctrl_click(ctx, state, style, id, center);
        assert_eq!(
            state[outer].is_horizontal(),
            !was_horizontal,
            "ctrl+clicking the handle did not flip the grouping"
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
        // Start as two columns, each with its own inner divider (node 0 is a horizontal split).
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
                modifiers: Modifiers::COMMAND,
            }]
        };
        // Two warm-up frames, not one — see `click_holding`'s doc for why a crossing's handle
        // needs both before it can be hit at all.
        for _ in 0..2 {
            run_frame(
                &ctx,
                &mut state,
                &style,
                id,
                vec![
                    Event::ModifiersChanged(Modifiers::COMMAND),
                    Event::PointerMoved(center),
                ],
            );
        }
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

    /// A style whose catch and hold margins are wide enough to aim between, for the two tests that
    /// are about the *mechanism* — a press that misses the square is still the button, and the zone
    /// that keeps the pointer is wider than the zone that caught it.
    ///
    /// Written down rather than taken from the defaults, and that is the lesson of trimming them:
    /// the shipped margins are a **feel**, tuned against a screen and a hand (1.0 and 0.5 as of
    /// 2026-08-10, from 6.0 and 6.0), while "there are two radii and the outer one holds" is a
    /// rule. A test that aims at `catch + hold - 1` reads as being about the rule and is really
    /// about the numbers: at half a point of hysteresis the band between the two radii is thinner
    /// than the pixel grid it is measured on, and the tests went red on a change that broke
    /// nothing.
    fn wide_toggle_style() -> Style {
        let mut style = band_style();
        style.cross_split_toggle.size = 14.0;
        style.cross_split_toggle.catch_extra = 6.0;
        style.cross_split_toggle.hold_extra = 6.0;
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

    /// A "+" whose picture cannot be rebuilt is not offered a *transposition*.
    ///
    /// The separator margin is a floor on how close a boundary may come to either end of the
    /// interval it is cut from, so on an interval shorter than twice the margin the boundary is
    /// pinned to the middle — and the parts that come out are shorter than the margin itself. A
    /// transposition re-cuts those parts from *different* intervals, and a fraction that lands
    /// outside the new interval's band is drawn clamped: the picture would jump.
    ///
    /// So the same scene is built twice and the margin is the only thing that differs. With a
    /// small one the toggle is there; with the default 175 px it is not, and the second half is
    /// what this test is for. The first half is what keeps it honest — without it, "nothing
    /// offered" would also be the answer for a scene that never had a crossing.
    ///
    /// What is *not* withdrawn is the handle: the junction is still a junction and a drag on it
    /// is meaningful — see [`Junctions::can_transpose`], which is why this gate stopped
    /// suppressing the detection and now only refuses the one gesture it is about.
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

    // ------------------------------------------------------------------------
    // The tee, and the drag that moves every separator meeting at a junction
    // ------------------------------------------------------------------------

    /// Where band `root`'s dividers sit on screen, ascending — the test's own reading of the
    /// boundaries [`Band::bounds`] names, taken off the rectangles rather than off the tree.
    fn divider_positions_of(
        ctx: &Context,
        state: &DockState<u32>,
        id: Id,
        root: NodeId,
        horizontal: bool,
    ) -> Vec<f32> {
        let layout = DockLayout::load(ctx, id);
        let rect = |n: NodeId| {
            layout
                .rect(NodePath::new(SurfaceIndex::main(), n))
                .expect("laid out this frame")
        };
        band_parts_of(state, root, horizontal)
            .windows(2)
            .map(|pair| {
                0.5 * (edge(rect(pair[0]), horizontal, true)
                    + edge(rect(pair[1]), horizontal, false))
            })
            .collect()
    }

    /// A handle is on screen where the pointer is, and nowhere else.
    ///
    /// There is one at every junction of every line, and each offers a thing you do *to the
    /// corner you are pointing at* — painted cold they are a grid of squares laid over the
    /// panels for no one. So the cold half is the assertion, and the warm half is what keeps it
    /// from passing on a handle that had stopped being drawn at all.
    ///
    /// Both halves rest the pointer for several frames: egui decides what is hovered from the
    /// rectangles registered during a frame, so the answer arrives one frame after the pointer.
    #[test]
    fn a_handle_is_drawn_only_under_the_pointer() {
        /// Well inside the top-left panel, far from any junction of this scene.
        const ELSEWHERE: Pos2 = Pos2::new(80.0, 80.0);

        let id = Id::new(DOCK_ID);
        let style = Style::default();
        let (ctx, mut state, outer, _) = tee_scene(&style, id);
        let at = junctions_on(&ctx, &mut state, &style, id, outer)[0].1;

        hover(&ctx, &mut state, &style, id, ELSEWHERE);
        let (_, cold) = run_frame_painting(&ctx, &mut state, &style, id, vec![]);
        assert!(
            cold.is_empty(),
            "the junction drew a handle with the pointer at {ELSEWHERE:?}: {cold:?}"
        );

        hover(&ctx, &mut state, &style, id, at);
        let (_, warm) = run_frame_painting(&ctx, &mut state, &style, id, vec![]);
        assert_eq!(
            warm.len(),
            1,
            "the pointer is on the junction at {at:?} and this is what was drawn: {warm:?}"
        );
        assert!(
            (warm[0].center() - at).length() < 2.0,
            "the handle drawn for the junction at {at:?} is at {:?}",
            warm[0].center()
        );
    }

    /// The arrows inside the handle are **visible against the square they are drawn on** — under
    /// the host's theme, which is the case that was broken.
    ///
    /// Reported from the screen: «кнопка стала полностью белой рисоваться без рисок» (Стас,
    /// 2026-08-10). The square and the icon used to take the two ends of the separator's palette,
    /// and under [`Style::from_egui`] those are `widgets.hovered.fg_stroke` and
    /// `widgets.active.fg_stroke` — **gray(240) and white** in egui's dark theme. Two roles, one
    /// colour, no arrows. The icon has a colour of its own now
    /// ([`CrossSplitToggleStyle::icon_color`], the panel fill by default).
    ///
    /// So the scene is built with `Style::from_egui` and not with `Style::default()`, which is the
    /// whole point of it: the crate's own defaults (black / gray / white) are legible and every
    /// other test here uses them, so none of them could see this. The difference is measured as a
    /// per-channel gap rather than as inequality — two greys a pixel apart are "different" and
    /// still invisible.
    #[test]
    fn the_handle_icon_is_visible_against_its_square() {
        /// How far apart the two colours must be, per channel, to read as an icon rather than as a
        /// smudge. Generous: the failure this pins had a gap of 15.
        const GAP: i32 = 60;

        let id = Id::new(DOCK_ID);
        let style = Style::from_egui(&egui::Style::default());
        let (ctx, mut state, outer, _) = tee_scene(&style, id);
        let at = junctions_on(&ctx, &mut state, &style, id, outer)[0].1;

        hover(&ctx, &mut state, &style, id, at);
        let (square, strokes) = handle_paint(&ctx, &mut state, &style, id);
        let square = square.expect("the pointer is on the junction, so a handle was painted");
        assert!(
            !strokes.is_empty(),
            "the handle's square was painted with no arrows on it at all"
        );

        let channels = |c: egui::Color32| [c.r() as i32, c.g() as i32, c.b() as i32];
        let (sq, sqc) = (square.1, channels(square.1));
        for stroke in strokes {
            let gap = channels(stroke)
                .iter()
                .zip(&sqc)
                .map(|(a, b)| (a - b).abs())
                .max()
                .unwrap();
            assert!(
                gap >= GAP,
                "an arrow is {stroke:?} on a {sq:?} square — {gap} apart per channel, which is a \
                 white square with nothing visible on it"
            );
        }
    }

    /// A crossing's handle is there under the pointer, ctrl or no ctrl.
    ///
    /// It used to appear only while ctrl was held, because the only gesture it had was the
    /// ctrl+click and a handle with no gesture takes the point away from the separators under it
    /// (egui drops the layers behind a widget covering the pointer — see `draw_one_handle`). A
    /// crossing is dragged now, so it has a gesture with the hand empty and the rule is satisfied
    /// the other way round: «+ не появляется. мы же условились что в целом её таскать можно»
    /// (Стас, 2026-08-10).
    ///
    /// The cold half is the control — the pointer well away from the crossing draws nothing, or
    /// "there is a handle" would also be true of a dock that paints a grid of squares over the
    /// panels.
    #[test]
    fn a_crossing_has_a_handle_with_no_modifier_held() {
        /// Well inside a panel, far from any junction of this scene.
        const ELSEWHERE: Pos2 = Pos2::new(80.0, 80.0);

        let id = Id::new(DOCK_ID);
        let style = Style::default();
        let (ctx, mut state, _, center) = cross_scene(&style, id);

        hover(&ctx, &mut state, &style, id, ELSEWHERE);
        let (_, cold) = run_frame_painting(&ctx, &mut state, &style, id, vec![]);
        assert!(
            cold.is_empty(),
            "the crossing drew a handle with the pointer at {ELSEWHERE:?}: {cold:?}"
        );

        hover(&ctx, &mut state, &style, id, center);
        let (_, open_hand) = run_frame_painting(&ctx, &mut state, &style, id, vec![]);
        assert_eq!(
            open_hand.len(),
            1,
            "an open-handed hover over the crossing at {center:?} drew {open_hand:?}"
        );

        // And ctrl does not change what is on screen — what it changes is what a click means.
        hover_holding(
            &ctx,
            &mut state,
            &style,
            id,
            center,
            egui::Modifiers::COMMAND,
        );
        let (_, with_ctrl) = run_frame_painting(&ctx, &mut state, &style, id, vec![]);
        assert_eq!(
            with_ctrl.len(),
            1,
            "ctrl is down and the pointer is on the crossing at {center:?}: {with_ctrl:?}"
        );
    }

    /// A drag has hold of one junction, and the ones it travels past are not part of it.
    ///
    /// The gesture is carried out on the nodes named at `drag_started` (see
    /// [`DragSubject::Junction`]), and every other handle stands down while it runs — so a drag
    /// that sweeps its
    /// divider across a neighbouring junction cannot pick that neighbour up, and cannot leave a
    /// second handle lit under the pointer either. Both are asserted: the neighbour's boundary
    /// where it was, and never two handles drawn in one frame.
    ///
    /// The control is the first assertion — the grabbed divider has to travel most of the way,
    /// or the sweep never reaches the neighbour and the rest is free.
    ///
    /// Measured, and worth saying plainly: this test is **green on the code that came before
    /// the explicit drag state too**, and green with the stand-down guard mutated out. egui hands
    /// one drag to one widget and suppresses hover on the rest, so neither hole was reachable from
    /// the outside — what the explicit state buys is that "what is being dragged" is a thing the
    /// dock says rather than a thing rederived per frame from geometry the drag is moving. This
    /// pins the contract, not the repair.
    #[test]
    fn a_drag_keeps_hold_of_the_junction_it_grabbed() {
        /// Far enough to carry the grabbed divider from 0.3 of the band across the other's 0.7.
        const DOWN: f32 = 380.0;

        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let style = band_style();
        let (mut state, outer_id, _) = build_bands(
            &ctx,
            &style,
            id,
            true,
            [&[0.3], &[0.7]],
            [Leaning::Left, Leaning::Left],
        );
        let outer = NodePath::new(SurfaceIndex::main(), outer_id);
        let [b0, b1] = state.main_surface().children(outer_id).unwrap();
        let positions = |state: &DockState<u32>| {
            (
                divider_positions_of(&ctx, state, id, b0, false)[0],
                divider_positions_of(&ctx, state, id, b1, false)[0],
            )
        };

        let junctions = junctions_on(&ctx, &mut state, &style, id, outer);
        assert_eq!(
            junctions.len(),
            2,
            "the scene is meant to be two tees on one line: {junctions:?}"
        );
        let at = junctions[0].1;
        let (before_grabbed, before_neighbour) = positions(&state);

        // Hand-run so every frame of the drag can be looked at, which is where "one handle at a
        // time" lives — a whole-gesture helper only shows the last one.
        use egui::{Event, Modifiers, PointerButton};
        let press = |pressed: bool, pos: Pos2| {
            vec![Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]
        };
        run_frame(&ctx, &mut state, &style, id, vec![Event::PointerMoved(at)]);
        run_frame(&ctx, &mut state, &style, id, press(true, at));
        for step in 1..=6u8 {
            let to = at + Vec2::new(0.0, DOWN * f32::from(step) / 6.0);
            let (_, painted) =
                run_frame_painting(&ctx, &mut state, &style, id, vec![Event::PointerMoved(to)]);
            assert!(
                painted.len() <= 1,
                "{} handles were drawn during the drag, at {painted:?}",
                painted.len()
            );
        }
        run_frame(
            &ctx,
            &mut state,
            &style,
            id,
            press(false, at + Vec2::new(0.0, DOWN)),
        );

        let (after_grabbed, after_neighbour) = positions(&state);
        assert!(
            after_grabbed - before_grabbed > DOWN * 0.5,
            "the grabbed divider moved {}pt of {DOWN}pt, so it never swept past the neighbour",
            after_grabbed - before_grabbed
        );
        assert!(
            (after_neighbour - before_neighbour).abs() < 1.0,
            "the junction the drag swept past moved with it: {before_neighbour} to \
             {after_neighbour}"
        );
    }

    /// A tee dragged **onto** a neighbouring tee — so that the two are one crossing for a frame —
    /// keeps its handle and keeps its pace.
    ///
    /// This is the reported bug, and the aiming is the whole scene. Halfway through such a sweep the
    /// grabbed divider lines up with the other band's, and while they are aligned to within
    /// `align_tolerance` the detector stops seeing two tees and sees **one crossing**: a different
    /// `kind`, a different key, a different widget id. The handle holding the gesture is then not
    /// drawn and not registered, egui goes on dragging a widget nobody answers for, and the resize
    /// stands still until the hand opens — «когда тройник пытается стать крестовиной там что-то
    /// происходит» (Стас, 2026-08-10).
    ///
    /// **The pointer is walked onto the neighbour's own position and held there**, rather than swept
    /// past it in even steps. Measured, and this is why the older
    /// `a_drag_keeps_hold_of_the_junction_it_grabbed` never caught the bug: with six even steps of
    /// 63pt the crossing window — one device pixel wide — falls between frames, and the mutation
    /// "no gesture takeover, no room gate" leaves that test green. Aimed at the neighbour, the same
    /// mutation reddens.
    ///
    /// Two things are asked every frame: **one** handle is on screen, and the boundary moved as far
    /// as the hand did. Per frame and not of the total, because a total tolerates a stall that is
    /// made up for afterwards — and per *frame* is what a stall on screen is.
    #[test]
    fn a_tee_dragged_onto_a_neighbour_keeps_its_handle_and_its_pace() {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let style = band_style();
        let (mut state, outer_id, _) = build_bands(
            &ctx,
            &style,
            id,
            true,
            [&[0.3], &[0.7]],
            [Leaning::Left, Leaning::Left],
        );
        let outer = NodePath::new(SurfaceIndex::main(), outer_id);
        let [b0, b1] = state.main_surface().children(outer_id).unwrap();
        let grabbed = |state: &DockState<u32>| divider_positions_of(&ctx, state, id, b0, false)[0];
        let neighbour =
            |state: &DockState<u32>| divider_positions_of(&ctx, state, id, b1, false)[0];

        let junctions = junctions_on(&ctx, &mut state, &style, id, outer);
        assert_eq!(
            junctions.len(),
            2,
            "the scene is meant to be two tees on one line: {junctions:?}"
        );
        let at = junctions[0].1;
        let target = junctions[1].1;
        let before_neighbour = neighbour(&state);

        // Half way, onto the neighbour, two frames standing exactly on it, then past it. The two
        // still frames are the crossing: the hand is not moving, so a boundary that moves is as
        // wrong as one that stalls while it does.
        let gap = target.y - at.y;
        let route: Vec<f32> = vec![
            at.y + gap * 0.5,
            target.y,
            target.y,
            target.y,
            target.y + gap * 0.5,
            target.y + gap,
        ];

        use egui::{Event, Modifiers, PointerButton};
        let press = |pressed: bool, pos: Pos2| {
            vec![Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            }]
        };
        run_frame(&ctx, &mut state, &style, id, vec![Event::PointerMoved(at)]);
        run_frame(&ctx, &mut state, &style, id, press(true, at));

        // The widget the gesture is begun under, read on the first travelling frame rather than on
        // the press: egui calls a press a *drag* only once the pointer has moved, so the field is
        // empty on the frame the button goes down.
        let mut held: Option<DragInFlight> = None;
        let mut aligned_frames = 0u8;
        let mut was = grabbed(&state);
        let mut hand = at.y;
        let mut asked_last = 0.0_f32;
        for (step, &y) in route.iter().enumerate() {
            let (_, painted) = run_frame_painting(
                &ctx,
                &mut state,
                &style,
                id,
                vec![Event::PointerMoved(Pos2::new(at.x, y))],
            );
            assert_eq!(
                painted.len(),
                1,
                "frame {step} of the drag drew {} handles ({painted:?}) — the gesture's own handle \
                 must be on screen every frame of it, crossing or no crossing",
                painted.len()
            );

            let live = drag_in_flight(&ctx, id)
                .unwrap_or_else(|| panic!("frame {step}: the dock is holding nothing mid-drag"));
            match held {
                None => {
                    assert!(
                        matches!(live.subject, DragSubject::Junction { .. }),
                        "the scene has to be a junction gesture: {:?}",
                        live.subject
                    );
                    held = Some(live);
                }
                Some(held) => {
                    assert_eq!(
                        live.widget, held.widget,
                        "frame {step}: the gesture changed widgets mid-flight"
                    );
                    assert_eq!(
                        format!("{:?}", live.subject),
                        format!("{:?}", held.subject),
                        "frame {step}: the gesture changed its subject mid-flight"
                    );
                }
            }

            let now = grabbed(&state);
            let travelled = now - was;
            // Compared against what the hand did on the **previous** frame, and that one frame is
            // structural rather than a fudge: `drag_junction` writes the split's fraction after this
            // pass has already laid the surface out, so a boundary read off the geometry map is a
            // frame behind the pointer for the whole gesture, not only at its start. Measured
            // (delta 176.5 on the pass the hand moved, the same 177pt appearing in the layout on
            // the next one) rather than assumed, and it is why the comparison is shifted instead of
            // being loosened.
            //
            // Skipped on the first two frames: the press's own travel arrives with the first
            // dragged frame, so neither pairs up one-to-one.
            if step >= 2 {
                assert!(
                    (travelled - asked_last).abs() < 1.5,
                    "frame {step}: the hand moved {asked_last}pt on the frame before and the \
                     boundary moved {travelled}pt on this one — a stall in the middle of a gesture"
                );
            }
            asked_last = y - hand;
            if (now - neighbour(&state)).abs() < 2.0 {
                aligned_frames += 1;
            }
            (was, hand) = (now, y);
        }
        run_frame(
            &ctx,
            &mut state,
            &style,
            id,
            press(false, Pos2::new(at.x, *route.last().unwrap())),
        );

        // The scene has to *be* about a crossing, and this is the gate that says so: some frame of
        // it had the two dividers on one line, which is when the detector's answer changes shape.
        assert!(
            aligned_frames > 0,
            "no frame of this drag had the grabbed divider on the neighbour's line, so the scene \
             never reached the crossing it is named after"
        );
        assert!(
            (neighbour(&state) - before_neighbour).abs() < 1.0,
            "the junction the drag swept past moved with it: {before_neighbour} to {}",
            neighbour(&state)
        );
    }

    /// A column beside a stack — `H(A, V(B, C))`. The stack's divider ends on the line between
    /// the two children: three panels meet there, and nothing crosses.
    ///
    /// Returns the state, `outer`'s path, and the leaves as `[left, top_right, bottom_right]`.
    fn tee_scene(style: &Style, id: Id) -> (Context, DockState<u32>, NodePath, [NodeId; 3]) {
        let ctx = Context::default();
        let mut state = DockState::new(vec![0u32]);
        let left = state.main_surface().root().unwrap();
        let [_, top_right] = state.split(
            NodePath::new(SurfaceIndex::main(), left),
            Split::Right,
            0.5,
            Node::leaf(1u32),
        );
        let outer = NodePath::new(
            SurfaceIndex::main(),
            state.main_surface().root().expect("the dock has a root"),
        );
        let [_, bottom_right] = state.split(
            NodePath::new(SurfaceIndex::main(), top_right),
            Split::Below,
            0.5,
            Node::leaf(2u32),
        );
        render(&ctx, &mut state, style, id);
        (ctx, state, outer, [left, top_right, bottom_right])
    }

    /// Three panels meet where one band's divider ends on the line, and that is a junction with
    /// a handle on it — the shape the detector used to walk straight past.
    ///
    /// The kind is asserted and not only the count, because "one junction here" is also true of
    /// a detector that called this a crossing: what a crossing claims is that *both* sides are
    /// divided, and this scene's left side is one undivided panel. And the point is checked
    /// against the two separators as drawn, so the handle cannot sit where they do not meet.
    #[test]
    fn a_tee_is_offered_where_only_one_band_is_divided() {
        let id = Id::new(DOCK_ID);
        let style = Style::default();
        let (ctx, mut state, outer, [left, top_right, bottom_right]) = tee_scene(&style, id);

        let junctions = junctions_on(&ctx, &mut state, &style, id, outer);
        assert_eq!(
            junctions.len(),
            1,
            "exactly one point in this scene has separators meeting at it: {junctions:?}"
        );
        assert_eq!(
            junctions[0].0,
            JunctionKind::Tee {
                side: 1,
                divider: 0
            },
            "the divider that ends on the line is the second band's first one"
        );
        assert!(
            toggle_centers(&ctx, &mut state, &style, id, outer).is_empty(),
            "nothing crosses here, so there is no grouping to transpose"
        );

        let layout = DockLayout::load(&ctx, id);
        let rect = |n: NodeId| {
            layout
                .rect(NodePath::new(SurfaceIndex::main(), n))
                .expect("laid out this frame")
        };
        let meeting = pos2(
            0.5 * (rect(left).right() + rect(top_right).left()),
            0.5 * (rect(top_right).bottom() + rect(bottom_right).top()),
        );
        assert!(
            (junctions[0].1 - meeting).length() < 1.0,
            "the handle is at {:?}, the separators meet at {meeting:?}",
            junctions[0].1
        );
    }

    /// The staggered "L" — two bands divided at *different* places. It has no crossing, and it
    /// never had; what it does have is two tees, one per divider ending on the line.
    ///
    /// This scene used to be the whole "the detector must not invent a cross" case and was
    /// asserted as "no button at all". Both halves are here now, because they are two different
    /// claims and only one of them was ever true of the picture.
    #[test]
    fn the_staggered_l_shape_has_two_tees_and_no_crossing() {
        let mut state = DockState::new(vec![0u32]);
        let a = state.main_surface().root().unwrap();
        let [_, node_c] = state.split(
            NodePath::new(SurfaceIndex::main(), a),
            Split::Right,
            0.5,
            Node::leaf(2u32),
        );
        let outer = NodePath::new(SurfaceIndex::main(), state.main_surface().root().unwrap());
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
        let style = Style::default();
        render(&ctx, &mut state, &style, id);

        let junctions = junctions_on(&ctx, &mut state, &style, id, outer);
        let kinds: Vec<JunctionKind> = junctions.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(
            kinds,
            vec![
                JunctionKind::Tee {
                    side: 0,
                    divider: 0
                },
                JunctionKind::Tee {
                    side: 1,
                    divider: 0
                },
            ],
            "two dividers end on this line, at different heights, and neither meets the other"
        );
        assert!(
            junctions[0].1.y < junctions[1].1.y,
            "junctions come out in screen order: {junctions:?}"
        );
    }

    /// The gesture: one drag on a tee moves **both** separators that meet at it — the line that
    /// runs through it, and the one that stops on it.
    ///
    /// Judged on the screen rather than on fractions: the two boundaries are cut from different
    /// intervals, so "the fraction changed" says nothing about how far anything went, and how far
    /// it went is the whole promise. The third assertion is what keeps the first two from passing
    /// on a gesture that moved the entire layout: the panel the stem does not touch keeps its
    /// height.
    #[test]
    fn dragging_a_tee_moves_both_of_its_separators() {
        const BY: Vec2 = Vec2::new(70.0, 45.0);

        let id = Id::new(DOCK_ID);
        let style = Style::default();
        let (ctx, mut state, outer, leaves) = tee_scene(&style, id);

        let at = junctions_on(&ctx, &mut state, &style, id, outer)[0].1;
        let before = leaf_rects(&DockLayout::load(&ctx, id), &leaves);
        drag(&ctx, &mut state, &style, id, at, at + BY);
        let after = leaf_rects(&DockLayout::load(&ctx, id), &leaves);

        assert!(
            (after[0].right() - before[0].right() - BY.x).abs() < 1.5,
            "the line between the two children was to move {}pt sideways, and went from {} to {}",
            BY.x,
            before[0].right(),
            after[0].right()
        );
        assert!(
            (after[1].bottom() - before[1].bottom() - BY.y).abs() < 1.5,
            "the divider ending on it was to move {}pt down, and went from {} to {}",
            BY.y,
            before[1].bottom(),
            after[1].bottom()
        );
        assert!(
            (after[0].height() - before[0].height()).abs() < 0.5,
            "the panel on the other side of the line is not divided and must keep its height"
        );
    }

    /// The gesture at a crossing: one drag moves the line **and both** dividers that end on it,
    /// so the four panels around it are resized together and the two dividers stay one line.
    ///
    /// This is the reverse of what the crate did until 2026-08-10, when a press at a crossing was
    /// deliberately a press on the separator underneath — the argument being that two dividers
    /// aligned by coincidence are not a corner. Overruled from the screen («в целом её таскать
    /// можно»), so the assertion is inverted rather than deleted: what used to be checked as a
    /// *sameness* with a press 200pt down the same line is now checked as a difference from it, and
    /// that control drag is still here, because "the crossing moved two dividers" says nothing if
    /// an ordinary separator drag moves them too.
    ///
    /// Judged on the screen and not on fractions: the three boundaries are cut from different
    /// intervals, so "the fraction changed" says nothing about how far anything went.
    #[test]
    fn dragging_a_crossing_moves_the_line_and_both_dividers() {
        const BY: Vec2 = Vec2::new(-60.0, 55.0);
        /// Far enough down the line to be clear of the handle's reach at any style.
        const CLEAR: f32 = 200.0;

        let id = Id::new(DOCK_ID);
        let style = Style::default();

        // A scene per gesture: the first drag is a real edit, and what the second is about is
        // where it was aimed, not the layout the first one left. `[top_left, .., bottom_right]`.
        let scene = |from: Option<Vec2>| {
            let ctx = Context::default();
            let (mut state, outer_id, leaves) = build_cross(true, 0.5, 0.5);
            let outer = NodePath::new(SurfaceIndex::main(), outer_id);
            render(&ctx, &mut state, &style, id);
            let at = toggle_center(&ctx, &mut state, &style, id, outer).expect("a 2x2 is a cross")
                + from.unwrap_or(Vec2::ZERO);
            let before = leaf_rects(&DockLayout::load(&ctx, id), &leaves);
            drag(&ctx, &mut state, &style, id, at, at + BY);
            (before, leaf_rects(&DockLayout::load(&ctx, id), &leaves))
        };

        let (before_at, after_at) = scene(None);
        let (before_clear, after_clear) = scene(Some(Vec2::new(0.0, CLEAR)));

        // The line itself, from either grab: this is the component both gestures share.
        assert!(
            (after_at[0].right() - before_at[0].right() - BY.x).abs() < 1.5,
            "the line between the two columns was to move {}pt sideways, and went from {} to {}",
            BY.x,
            before_at[0].right(),
            after_at[0].right()
        );
        assert!(
            (after_clear[0].right() - before_clear[0].right() - BY.x).abs() < 1.5,
            "the control drag, {CLEAR}pt clear of the crossing, did not move the line it grabbed"
        );

        // What only the crossing does: both dividers follow, on either side of the line.
        for (panel, corner) in [(0usize, "left"), (2usize, "right")] {
            assert!(
                (after_at[panel].bottom() - before_at[panel].bottom() - BY.y).abs() < 1.5,
                "the {corner} column's divider was to move {}pt down with the crossing, and went \
                 from {} to {}",
                BY.y,
                before_at[panel].bottom(),
                after_at[panel].bottom()
            );
            // And the control leaves them where they were, which is what makes the four lines
            // above a fact about the crossing rather than about dragging in general.
            assert!(
                (after_clear[panel].bottom() - before_clear[panel].bottom()).abs() < 0.5,
                "a drag {CLEAR}pt clear of the crossing carried the {corner} column's divider \
                 along: {} to {}",
                before_clear[panel].bottom(),
                after_clear[panel].bottom()
            );
        }
    }

    /// A plain click transposes nothing.
    ///
    /// That is the price of the drag sharing the handle with the toggle: a press that was meant
    /// to move a line and did not travel far enough arrives as a click, and a click that rewrote
    /// the tree would turn every short drag into a regrouping. The control is the same press with
    /// ctrl held, which must still flip — otherwise "nothing happened" would also be the answer
    /// for a handle that had stopped working.
    #[test]
    fn a_plain_click_on_a_crossing_transposes_nothing() {
        let id = Id::new(DOCK_ID);
        let style = band_style();
        let (ctx, mut state, outer, center) = cross_scene(&style, id);

        let was_horizontal = state[outer].is_horizontal();
        click(&ctx, &mut state, &style, id, center);
        assert_eq!(
            state[outer].is_horizontal(),
            was_horizontal,
            "a click with no modifier transposed the grouping"
        );
        assert!(
            click_flips(&ctx, &mut state, &style, id, outer, center),
            "ctrl+clicking the same point did nothing either, so the assertion above is free"
        );
    }

    /// One junction drag, one commit.
    ///
    /// A single-separator drag reports a stream of [`DockEvent::SeparatorDragging`] while it runs
    /// and exactly one [`DockEvent::LayoutCommitted`] when it ends, and a consumer that persists
    /// the layout or drives undo is written against that shape. A gesture that moves two
    /// separators has to report itself the same way — the alternative is a layout change that no
    /// consumer hears about, or one they hear about twice.
    #[test]
    fn a_junction_drag_reports_itself_like_a_separator_drag() {
        let id = Id::new(DOCK_ID);
        let style = Style::default();
        let (ctx, mut state, outer, _) = tee_scene(&style, id);

        let at = junctions_on(&ctx, &mut state, &style, id, outer)[0].1;
        let reported = drag(&ctx, &mut state, &style, id, at, at + Vec2::new(40.0, 30.0));

        let dragging = reported
            .iter()
            .filter(|event| matches!(event, DockEvent::SeparatorDragging))
            .count();
        let committed = reported.iter().filter(|event| event.is_committed()).count();
        assert!(
            dragging > 0,
            "the drag moved the layout without saying so: {reported:?}"
        );
        assert_eq!(
            committed, 1,
            "one finished gesture is one commit, and this reported {committed}: {reported:?}"
        );
    }

    /// Every node of the main surface, split by kind.
    fn nodes_by_kind(state: &DockState<u32>) -> (Vec<NodeId>, Vec<NodeId>) {
        let mut leaves = Vec::new();
        let mut splits = Vec::new();
        for node in state.main_surface().breadth_first() {
            if state[NodePath::new(SurfaceIndex::main(), node)].is_leaf() {
                leaves.push(node);
            } else {
                splits.push(node);
            }
        }
        leaves.sort_unstable();
        splits.sort_unstable();
        (leaves, splits)
    }

    /// What a transposition does promise about ids.
    ///
    /// Not "every id names what it named" — see the test after this one for why that is
    /// impossible. What holds is the bookkeeping: the same leaves, the same *set* of split ids
    /// (nothing created, nothing dropped — the rebuild is handed exactly the splits it took
    /// apart), and the crossing's own node still where its parent points.
    ///
    /// Worth pinning because the rebuild consumes its splits from a pool, and a pool is exactly
    /// the shape of thing that leaks one or invents one when the counting is wrong. The `assert`
    /// inside `transpose_cross_split` catches a leftover; this catches the other direction, and
    /// catches it on the tree rather than on the arithmetic.
    #[test]
    fn a_transposition_creates_and_destroys_no_nodes() {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let style = band_style();

        let (mut state, outer_id, _) = build_bands(
            &ctx,
            &style,
            id,
            false,
            [&[0.4], &[0.4, 0.75]],
            [Leaning::Left, Leaning::Left],
        );
        let outer = NodePath::new(SurfaceIndex::main(), outer_id);

        let before = nodes_by_kind(&state);
        press_toggle(&ctx, &mut state, &style, id, outer);
        let after = nodes_by_kind(&state);

        assert_eq!(before.0, after.0, "the set of leaves changed");
        assert_eq!(before.1, after.1, "the set of splits changed");
        assert!(
            state.main_surface().root() == Some(outer_id),
            "the crossing's own node lost its place in the tree"
        );
    }

    /// And what it cannot promise: which line a split id names.
    ///
    /// This is a counting fact, not a bug to be fixed one day, and it is a test so that nobody
    /// "fixes" it by accident and so that the guarantee above is not read as more than it says.
    /// The line the two bands shared was one divider in each of them — two nodes — and comes
    /// back as one divider spanning both. The old outer boundary was one node and comes back as
    /// two, one per half. Two into one and one into two: no assignment of the old ids to the new
    /// boundaries can keep every one of them on the segment it was drawn on.
    ///
    /// The visible form of it is orientation: at least one split that was cutting one way is
    /// cutting the other way afterwards, while still carrying its old id.
    #[test]
    fn a_transposition_may_hand_a_split_id_a_line_of_the_other_orientation() {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        let style = band_style();

        let (mut state, outer_id, _) = build_bands(
            &ctx,
            &style,
            id,
            false,
            [&[0.4], &[0.4, 0.75]],
            [Leaning::Left, Leaning::Left],
        );
        let outer = NodePath::new(SurfaceIndex::main(), outer_id);

        let orientation = |state: &DockState<u32>| {
            let (_, splits) = nodes_by_kind(state);
            splits
                .iter()
                .map(|node| {
                    (
                        *node,
                        state[NodePath::new(SurfaceIndex::main(), *node)].is_horizontal(),
                    )
                })
                .collect::<Vec<_>>()
        };

        let before = orientation(&state);
        press_toggle(&ctx, &mut state, &style, id, outer);
        let after = orientation(&state);

        let turned = before
            .iter()
            .zip(&after)
            .filter(|((_, was), (_, now))| was != now)
            .count();
        assert!(
            turned > 0,
            "not one split id changed the orientation of the line it names. Either the scene \
             stopped being the one this is about, or ids became stable across a transposition — \
             in which case this test should become the guarantee, not the caveat"
        );
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
