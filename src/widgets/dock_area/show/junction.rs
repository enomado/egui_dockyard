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

use egui::{
    CursorIcon, Id, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
    epaint::CornerRadius, pos2, vec2,
};

use super::glyph;
use crate::dock_area::DockMutation;
use crate::dock_area::events::DockEvent;
use crate::dock_area::state::{DragSubject, JunctionArms, State};
use crate::{CrossSplitToggleStyle, DockArea, GapPath, NodePath, SeparatorStyle};

/// One side of a split, with the chain of same-orientation splits at its root flattened: `n`
/// parts side by side, with `n - 1` dividers between them.
///
/// The flattening stops at the first node of the *other* orientation, and that is not an
/// approximation — a divider below such a node does not span the band, so it cannot reach
/// either end of it and cannot take part in a crossing.
/// What the parts themselves are is not here: this is the *measured* half of a chain, and which
/// nodes make it up is the tree's half — [`Tree::chain`](crate::core::tree::Tree::chain), which
/// `band` calls and a transposition calls again when it is applied.
struct Band {
    /// The chain's gaps, in screen order: `dividers[k]` is the gap whose boundary falls
    /// between part `k` and part `k + 1`.
    dividers: Vec<GapPath>,

    /// The `dividers.len() + 2` boundaries along the band's own axis, ascending: the band's two
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

/// Every junction on the line in one gap of a row, together with the two bands they were found
/// in — which the drag and the transposition both need, so they are kept rather than re-derived.
struct Junctions {
    /// The gap whose line the junctions sit on: between two neighbouring children of a row.
    outer: GapPath,

    /// Orientation of `outer`'s row: `true` if it lays its children out side by side.
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

/// One edge of a rectangle along an axis: `x` for a horizontal one, `y` for a vertical one.
fn edge(rect: Rect, horizontal: bool, far: bool) -> f32 {
    match (horizontal, far) {
        (true, false) => rect.min.x,
        (true, true) => rect.max.x,
        (false, false) => rect.min.y,
        (false, true) => rect.max.y,
    }
}

// The rebuild itself — which node ends up under which split — lives on the tree, in
// `core::tree::transpose`: it is an operation on the tree, and the only thing this module has
// that the tree does not is where the boundaries ended up on screen.

impl<Tab> DockArea<'_, Tab> {
    /// Flattens the chain of `horizontal`-oriented splits rooted at `root` into a [`Band`].
    ///
    /// `None` if the geometry map does not describe every part, or if one of them is degenerate
    /// (zero-size): such a part has nothing to pivot around, and dividing by its extent would
    /// produce a NaN fraction — which this crate has already been bitten by once (see the project's
    /// incident notes on `SplitNode.fraction`).
    fn band(&self, root: NodePath, horizontal: bool) -> Option<Band> {
        // Which nodes make up the chain is a question about the tree, and the tree answers it —
        // the same walk `transpose_cross` uses to rebuild them. What is added here is the half
        // the tree cannot know: where they ended up on screen.
        let chain = self.dock_state[root.surface].chain(root.node, horizontal);

        let rects: Vec<Rect> = chain
            .parts
            .iter()
            .map(|node| self.layout.rect(NodePath::new(root.surface, *node)))
            .collect::<Option<_>>()?;
        if rects.iter().any(|r| r.width() <= 0.0 || r.height() <= 0.0) {
            return None;
        }

        let mut bounds = Vec::with_capacity(rects.len() + 1);
        bounds.push(edge(rects[0], horizontal, false));
        for pair in rects.windows(2) {
            bounds.push(0.5 * (edge(pair[0], horizontal, true) + edge(pair[1], horizontal, false)));
        }
        bounds.push(edge(rects[rects.len() - 1], horizontal, true));

        Some(Band {
            dividers: chain
                .dividers
                .into_iter()
                .map(|gap| GapPath::in_surface(root.surface, gap))
                .collect(),
            bounds,
        })
    }

    /// Every junction on the line in `outer` — between the two children of its row that the gap
    /// lies between — in screen order.
    ///
    /// `outer` must already be known to name a gap of a row; callers of `show_divider` establish
    /// that before this runs.
    ///
    /// How far out of line two dividers may be and still be one crossing rather than two tees
    /// comes from `toggle`; `pixels_per_point` is what puts the floor under it in the points
    /// this geometry is measured in — see [`Junctions::tolerance`].
    fn detect_junctions(
        &self,
        outer: GapPath,
        extra: f32,
        toggle: &CrossSplitToggleStyle,
        pixels_per_point: f32,
    ) -> Option<Junctions> {
        let outer_horizontal = self.dock_state[outer.row].is_horizontal();
        // The line these junctions would sit *on* has to be a line. A row whose child was
        // folded away is cut at the strip's edge instead of at its ratio, and the layout then
        // draws no divider in that gap at all (see `DockLayout::divider`) — there is nothing
        // here for a junction to meet, and nothing for a hand to take hold of.
        self.separator_rect(outer)?;
        let [c0, c1] = self.gap_neighbours(outer);

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
        // A junction is a meeting of boundaries a hand can take hold of, and a band knows only
        // where its parts ended up on screen. A part folded away leaves its split cut at the
        // strip's edge, so the layout draws no divider for it — and a handle offered there is not
        // a cosmetic slip: the press is answered and the drag begins, then `follow_held_junction`
        // asks the same layout for that rectangle, finds none, draws nothing, and the gesture is
        // dropped with the button still down. The corner goes dead until the hand opens, and the
        // ratio the folded panel is keeping for its return would have been the thing being
        // dragged. Found by the DST sweep at seed 5, the pass it learned to fold.
        //
        // Each position is carried with the index it had in its band, rather than the list being
        // filtered afterwards: the kinds below name a band's own divider, and a crossing whose
        // other half is not drawn has to degrade into the tee it visibly is instead of vanishing
        // along with it.
        let drawn = |band: &Band| -> Vec<(usize, f32)> {
            band.divider_positions()
                .iter()
                .copied()
                .enumerate()
                .filter(|(index, _)| self.separator_rect(band.dividers[*index]).is_some())
                .collect()
        };
        let (first, second) = (drawn(&band0), drawn(&band1));
        let tolerance = Junctions::tolerance(toggle, pixels_per_point);
        let (mut i, mut j) = (0, 0);
        let mut at = Vec::new();
        while i < first.len() || j < second.len() {
            let pair = first.get(i).zip(second.get(j));
            if let Some((&(first_divider, a), &(second_divider, b))) = pair
                && (a - b).abs() <= tolerance
            {
                at.push(Junction {
                    kind: JunctionKind::Cross([first_divider, second_divider]),
                    center: at_line(0.5 * (a + b)),
                });
                i += 1;
                j += 1;
                continue;
            }
            // Screen order: whichever of the two heads comes first along the line is the next
            // junction, and the list that ran out has no head at all.
            let take_first = pair.is_none_or(|(a, b)| a.1 < b.1) && i < first.len();
            if take_first {
                at.push(Junction {
                    kind: JunctionKind::Tee {
                        side: 0,
                        divider: first[i].0,
                    },
                    center: at_line(first[i].1),
                });
                i += 1;
            } else {
                at.push(Junction {
                    kind: JunctionKind::Tee {
                        side: 1,
                        divider: second[j].0,
                    },
                    center: at_line(second[j].1),
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
    fn handle_room(&self, junctions: &Junctions, index: usize) -> f32 {
        let Junction { kind, center } = junctions.at[index];
        let surface = junctions.outer.row.surface;
        let own: Vec<GapPath> = std::iter::once(junctions.outer)
            .chain(
                kind.dividers()
                    .map(|(band, k)| junctions.bands[band].dividers[k]),
            )
            .collect();

        let mut room = junctions.room_at(index);
        // Every gap of every row of the surface: a divider is a gap's, and a row has as many as
        // it has neighbouring pairs of children.
        for node in self.dock_state[surface].breadth_first() {
            let path = NodePath::new(surface, node);
            let Some(row) = self.dock_state[path].get_row() else {
                continue;
            };
            for gap in row.gaps() {
                let gap = GapPath::new(path, gap);
                if own.contains(&gap) {
                    continue;
                }
                let Some(divider) = self.separator_rect(gap) else {
                    continue;
                };
                room = room.min(square_gap(divider, center));
            }
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
        outer: GapPath,
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
                self.request_transpose_cross_split(&junctions, index);
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
        outer: GapPath,
        outer_horizontal: bool,
        arms: JunctionArms,
        style: &SeparatorStyle,
        toggle: &CrossSplitToggleStyle,
        state: &mut State,
    ) -> Grip {
        let pass = ui.ctx().cumulative_pass_nr();

        // The subject can leave the tree under the hand — a leaf closed mid-gesture takes the
        // splits above it with it — and this is where that is noticed. It used to have to be
        // asked of the tree *before* the geometry, because `separator_rect` indexed the node and
        // panicked on a path that names nothing (`no node 0.1 in this tree`, which is how the
        // sweep reported this at seed 1, step 16); now that it is a lookup in the geometry map,
        // a dead path simply has no divider and the two `else` arms below cover it. The explicit
        // check stays because it says what this is *about* — the subject died — rather than
        // leaving that meaning to be inferred from a missing rectangle. Nothing is drawn and
        // nothing is reported; the field's own liveness filter drops the gesture a pass later,
        // which is the one divergence the harness checks rather than exempts.
        let divider = arms.first();
        if self.dock_state.node(outer.row).is_err()
            || arms
                .dividers()
                .iter()
                .any(|gap| self.dock_state.node(gap.row).is_err())
        {
            return Grip::Idle;
        }
        let Some(outer_rect) = self.separator_rect(outer) else {
            return Grip::Idle;
        };
        let Some(divider_rect) = self.separator_rect(divider) else {
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
                    glyph::barbed_arrow(ui.painter(), center, dir, toggle.size * 0.28, stroke);
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

        // Keyed by the gaps that meet here rather than by their position in the band: an id
        // has to survive a neighbouring divider being dragged past, and an index does not. The
        // side a tee's divider is on is already in the id it contributes; `outer` is in there
        // because a junction is a fact about *that* line, and two of them are told apart by it.
        let key: Vec<GapPath> = std::iter::once(junctions.outer)
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
        let room = self.handle_room(junctions, index);
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
                        glyph::barbed_arrow(ui.painter(), center, dir, handle_size * 0.28, stroke);
                    }
                }

                // What the gesture has hold of, named once and kept: the gaps, not an index
                // into a list that is rebuilt from the geometry this drag is about to move. See
                // [`DragSubject::Junction`].
                if response.drag_started() {
                    let mut dividers = kind
                        .dividers()
                        .map(|(band, k)| junctions.bands[band].dividers[k]);
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
                if response.drag_stopped()
                    && state.end_drag(handle_id).is_some_and(|drag| drag.moved)
                {
                    self.events.push(DockEvent::LayoutCommitted);
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
        outer: GapPath,
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

        // Each is clamped on its own by `nudge_boundary`: they are cut from intervals at right
        // angles to each other and have nothing to stay in line with. A crossing has two of them,
        // one on each side of the line, and both take the same component — which is what keeps
        // them one line on screen while the gesture runs.
        let mut moved = self.nudge_boundary(outer, pixels_per_point, extra, along_outer);
        for divider in arms.dividers() {
            moved |= self.nudge_boundary(*divider, pixels_per_point, extra, along_bands);
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
    /// The measuring half of a transposition: turn what this frame drew into the four numbers
    /// [`Tree::transpose_cross`](crate::core::tree::Tree::transpose_cross) needs, and queue it.
    ///
    /// Queued rather than done here, like every other edit drawing asks for. Its only caller is
    /// a toggle button drawn at the tail of `show_separator` for `outer`, in the *middle* of the
    /// separator pass — `show_separator` is still going to be called for the nodes below it
    /// further down the same loop, reading their geometry from [`Self::layout`]. Rewriting the
    /// tree under that loop is what used to force a mid-pass relayout to keep the two in step;
    /// with the edit in the epilogue the whole pass sees one shape, and the relayout happens
    /// once, after it, where `apply_render_mutations` puts it.
    ///
    /// What is left of the old hazard is a frame of dividers painted and hit-tested against the
    /// grouping the user has just replaced — the click frame, ~16 ms, the same shift already
    /// accepted for activation and collapsing. The geometry map published at the end of that
    /// frame still describes the tree that now exists, which is what
    /// `the_toggle_leaves_the_geometry_map_describing_the_tree_it_just_wrote` pins.
    fn request_transpose_cross_split(&mut self, junctions: &Junctions, index: usize) {
        let Junctions {
            outer,
            bands,
            outer_bounds,
            ..
        } = junctions;
        let [band0, band1] = bands;
        let JunctionKind::Cross([i, j]) = junctions.at[index].kind else {
            unreachable!("only a crossing is offered a transposition")
        };

        // The *row*, not the gap: a transposition regroups the two chains on either side of the
        // line, and the line of a pair is its only gap. A crossing on a row of three is a
        // different gesture, not a wider version of this one — see `Tree::transpose_cross`.
        self.mutations.push(DockMutation::TransposeCross {
            outer: *outer,
            at: [i, j],
            bounds: [band0.bounds.clone(), band1.bounds.clone()],
            // The one number from the other axis: where `outer`'s own boundary sits between its
            // two edges, which is the share each rebuilt half keeps for the first chain.
            stack_fraction: (outer_bounds[1] - outer_bounds[0])
                / (outer_bounds[2] - outer_bounds[0]),
        });
    }
}

#[cfg(test)]
mod tests;
