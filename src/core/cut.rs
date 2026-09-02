//! Cutting a row along its axis: how much of its length each child gets, and where a boundary
//! between two of them may sit.
//!
//! Two answers to the same question — *where along the axis* — and both are arithmetic over
//! `f32` with no rectangle, no scale factor and no style in them:
//!
//! * [`cut_runs`] turns a row of weights and fixed strips into the positions its children are
//!   cut at, and the gaps that get a divider;
//! * [`SeparatorBand`] says how far a gesture may move one of those boundaries, and where the
//!   stored ratio is honoured when the geometry cannot hold it.
//!
//! # Why it lives here
//!
//! It was written inside `DockArea::cut_row`, between the code that gathers a row's children and
//! the code that paints dividers, and could only be judged by rendering a frame and measuring the
//! result. Nothing here needs a `Ui`: the caller turns the answers into `egui::Rect`s, and the
//! rules themselves are decided by these functions alone. Here they are covered by the property
//! tests and by [`tests/core_is_egui_free.rs`](../../tests/core_is_egui_free.rs), which is the
//! difference the move was for.
//!
//! The caller keeps everything that *is* about the screen: which children a row has, what a
//! collapsed one costs in points, and which rectangle a span becomes.

/// How one child takes its length along the row's axis, as [`cut_runs`] sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Extent {
    /// Exactly this many points, whatever the row has to give: a collapsed child of a vertical
    /// row (its rows of tab bars — `collapsed_strip_height`), or a child of a horizontal row
    /// that fits in sideways strips (`collapsed_strip_width`).
    Fixed(f32),

    /// A share of what the fixed children leave, by this weight: an open child.
    Weighted(f32),
}

/// Where a child landed in a strip-aware cut — see [`cut_runs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Run {
    /// Among the fixed children pressed against the row's near edge (top / left), one after
    /// the other.
    Leading,

    /// Between the two runs: the open children sharing what is left, and any fixed child that
    /// sits among them.
    Middle,

    /// Among the fixed children pressed against the row's far edge (bottom / right).
    Trailing,
}

/// One child's place along the axis: the position it is cut at on each side, or `None` for the
/// row's own edge, left uncut.
pub(crate) type Span = (Option<f32>, Option<f32>);

/// What [`cut_runs`] decided, in one dimension.
pub(crate) struct RunCut {
    /// Per child, in order.
    pub(crate) spans: Vec<Span>,

    /// Per gap: the two edges of a divider to draw there, or `None` where the gap was cut at a
    /// fixed child's edge and there is no line at a ratio to draw or to grab.
    pub(crate) dividers: Vec<Option<(f32, f32)>>,

    /// Per child: which run it landed in.
    pub(crate) runs: Vec<Run>,
}

/// Cuts a row whose children do not all take a share: the fixed ones are given exactly their
/// length, and what is left goes to the open ones.
///
/// The one arithmetic behind two branches of `DockArea::cut_row`. A vertical row with a
/// collapsed child and a horizontal row with a sideways strip are the same problem one axis
/// over, and the pair-shaped code had solved it twice — which is how the 30.08 bug lived in one
/// axis and not the other (`a_row_collapses_panel_by_panel`).
///
/// **Runs.** Fixed children at the start of the row are stacked from its near edge (`lo`), one
/// after the other; fixed children at its end are stacked from its far edge (`hi`). Whatever
/// lies between — the open children, and any fixed child among them — shares the span the two
/// runs leave: a fixed child keeps its length, the open ones split the rest by weight (or
/// equally, if their weights add up to nothing: a proportion of nothing is not a proportion,
/// and an equal share is the least-bad answer, as [`SeparatorBand`] answers the centre when it
/// has no room to give). With no open child at all everything is one leading run, and
/// `last_fixed_takes_the_rest` says what happens to the row's far side: `false` cuts the last
/// child at its own length and the rest of the row is nobody's (the horizontal decision of
/// 30.08); `true` leaves it at the row's edge (the vertical rule the pair had).
///
/// **Snapping.** Every position handed back has been through `cut`, which puts it on the pixel
/// grid. `carry` is what a position goes through before the *next* one is computed from it, and
/// it is the one place the two axes differ — inherited, not chosen. The horizontal branch
/// snapped its run (`right_start = cut_at(left_end + separator)` with `left_end` already
/// snapped); the vertical one snapped each edge from the unsnapped run (`far = near + separator`
/// in points, then snapped). At an integer `pixels_per_point` the two agree; at a fractional
/// one they can land a pixel apart, so a parity stage hands each branch its own. Pinned by
/// `the_two_axes_snap_their_runs_differently_and_it_is_inherited`: unifying them is a decision
/// about pixels, not a cleanup.
///
/// **Dividers.** A divider is recorded wherever two open children have **only fixed children
/// between them**, and it is recorded at *both* edges of what lies between: one line where the
/// near open child ends, one where the far one begins. Two adjacent open children are the same
/// rule with nothing in between, and the two lines are then one — which is what every row
/// without a strip in it is, so nothing moves there.
///
/// Both lines mean the same trade: the two open children divide what the fixed ones leave, and
/// the strip between them rides along at its own width. A drag reads that pairing back out of
/// `DockArea::trading_pair`, which walks the same extents.
///
/// This used to be "only between two open neighbours", and a strip in the *middle* of a row then
/// killed both of its gaps at once: the two open columns either side of it had no line between
/// them anywhere, and no way to be resized against each other at all. A strip at the row's *end*
/// still has no line beside it, and wants none — there is only one open child there, and nothing
/// for it to trade with.
///
/// # Panics
///
/// If the row holds fewer than two children — a bug in the caller, which built `extents` from a
/// row.
pub(crate) fn cut_runs(
    lo: f32,
    hi: f32,
    extents: &[Extent],
    separator: f32,
    cut: impl Fn(f32) -> f32,
    carry: impl Fn(f32) -> f32,
    last_fixed_takes_the_rest: bool,
) -> RunCut {
    let n = extents.len();
    assert!(n >= 2, "a row of {n} has nothing to cut between");
    let is_open = |index: usize| matches!(extents[index], Extent::Weighted(_));
    let fixed = |index: usize| match extents[index] {
        Extent::Fixed(length) => length,
        Extent::Weighted(_) => unreachable!("child {index} is in a run of fixed children"),
    };

    // The leading run is everything before the first open child, the trailing run everything
    // after the last one. With no open child there is no trailing run: the whole row is the
    // leading one.
    let first_open = (0..n).find(|&index| is_open(index));
    let leading_end = first_open.unwrap_or(n);
    let trailing_start = match first_open {
        Some(_) => {
            (0..n)
                .rev()
                .find(|&index| is_open(index))
                .expect("an open child was found going forward")
                + 1
        }
        None => n,
    };

    let mut spans: Vec<Span> = vec![(None, None); n];
    let mut dividers = vec![None; n - 1];
    let mut runs = vec![Run::Middle; n];

    // Down from the near edge. `cursor` is where the next child begins, carried the way the
    // caller asked; each child's edges are snapped from it.
    let mut cursor = lo;
    for index in 0..leading_end {
        runs[index] = Run::Leading;
        let end = cursor + fixed(index);
        let last = index == n - 1;
        spans[index] = (
            (index > 0).then(|| cut(cursor)),
            (!(last && last_fixed_takes_the_rest)).then(|| cut(end)),
        );
        cursor = carry(carry(end) + separator);
    }
    let top = cursor;

    // Up from the far edge, the same thing mirrored.
    let mut cursor = hi;
    for index in (trailing_start..n).rev() {
        runs[index] = Run::Trailing;
        let start = cursor - fixed(index);
        spans[index] = (Some(cut(start)), (index < n - 1).then(|| cut(cursor)));
        cursor = carry(carry(start) - separator);
    }
    let bottom = cursor;

    // Between the runs.
    let middle = leading_end..trailing_start;
    if !middle.is_empty() {
        let fixed_total: f32 = middle
            .clone()
            .filter(|&index| !is_open(index))
            .map(fixed)
            .sum();
        let (open_count, weight_total) = middle
            .clone()
            .filter_map(|index| match extents[index] {
                Extent::Weighted(weight) => Some(weight),
                Extent::Fixed(_) => None,
            })
            .fold((0usize, 0.0f32), |(count, sum), weight| {
                (count + 1, sum + weight)
            });
        let separators = (middle.len() - 1) as f32 * separator;
        let free = (bottom - top - fixed_total - separators).max(0.0);

        let mut cursor = top;
        for index in middle.clone() {
            let length = match extents[index] {
                Extent::Fixed(length) => length,
                Extent::Weighted(weight) if weight_total > 0.0 => free * weight / weight_total,
                Extent::Weighted(_) => free / open_count as f32,
            };
            let end = cursor + length;
            let last = index == n - 1;
            let last_of_the_middle = index + 1 == trailing_start;
            spans[index] = (
                (index > 0).then(|| cut(cursor)),
                if last {
                    None
                } else if last_of_the_middle {
                    // Where the trailing run begins: `bottom` itself, and not `cursor + length`,
                    // which is the same number only up to rounding. On a pair this child is
                    // the whole middle, and that difference is the whole of parity here.
                    Some(cut(bottom))
                } else {
                    Some(cut(end))
                },
            );
            // A line at this gap when it is an *outer* edge of what separates two open
            // children: the near child is open and somebody open is still ahead (the near
            // edge of the run of strips), or the far child is open and somebody open is
            // behind (its far edge). With nothing in between both clauses name the same gap
            // and it gets its one line, exactly as before.
            //
            // Guarded by `last_of_the_middle` first and not merely also: past the middle there
            // is no gap `index` to record and no child `index + 1` in the middle to ask about.
            let spans_a_trade = !last_of_the_middle && {
                let open_behind = middle.clone().any(|k| k <= index && is_open(k));
                let open_ahead = middle.clone().any(|k| k > index && is_open(k));
                (is_open(index) && open_ahead) || (is_open(index + 1) && open_behind)
            };
            if spans_a_trade {
                dividers[index] = Some((cut(end), cut(carry(carry(end) + separator))));
            }
            cursor = carry(carry(end) + separator);
        }
    }

    RunCut {
        spans,
        dividers,
        runs,
    }
}

/// The band a row's boundary may occupy this frame, and where the stored ratio sits inside it.
///
/// [`SeparatorStyle::extra`](crate::SeparatorStyle::extra) is a margin in *pixels* that each
/// child must keep, so on a node `range` px long it is the fraction `extra / range`. Two things
/// come out of that, and keeping them apart is the whole reason this type exists:
///
/// * `min` / `max` — the limits a **gesture** may write between;
/// * `effective` — where the boundary is **drawn** and where the children are cut, which is the
///   stored ratio pushed into those limits *without being written back*.
///
/// The separation matters because the band depends on geometry and the ratio does not. Applying
/// the band to the stored ratio on every frame — which is what this code used to do, drag or
/// no drag — turns a window resize into a silent edit of the layout: on a node shorter than
/// `2 * extra` the band is the single point `0.5`, so the ratio the user set is replaced by dead
/// centre and growing the window back does not bring it home. A ratio is state; only a gesture
/// gets to change it. Geometry gets to decide where it is honoured.
///
/// # Everything that writes a boundary, and whether it asks
///
/// The clamp is applied when the boundary is *drawn* and is never written back, so a ratio the
/// band cannot hold is not an error anywhere — it is simply drawn somewhere else than it says.
/// That makes every writer a place where the tree and the screen can quietly part company, and
/// there are only four of them:
///
/// * `DockArea::nudge_boundary` — every gesture that moves a boundary, whether the drag and
///   the arrow keys in `DockArea::show_divider` or a drag on a junction handle, which moves two
///   or three of them at once. It clamps into `min..max`, so it asks. One function and not one
///   per gesture: a second copy of this arithmetic is a second answer to "how far may this go";
/// * the double-click in the same place — writes `0.5`, which is in every band there is, since
///   `min = (extra / range).min(0.5)` and `max = 1.0 - min`;
/// * `DockArea::transpose_cross_split`, the one writer that derives a fraction from measured
///   pixels — asks up front, for every fraction the rebuild will write, through
///   `Band::parts_can_be_renested`, and declines the whole gesture rather than move a pixel;
/// * `DockState::split` and friends, where the number comes from the caller.
///
/// Nothing derived from geometry escapes unasked today. What keeps that true is not this list —
/// lists go stale — but [`TreeViolation::RowShareNegative`](crate::TreeViolation), which catches
/// the arithmetic that answers outside the row it was measuring, wherever it is written from: a
/// boundary written past either end of a row leaves a negative weight on the child that lost.

#[derive(Clone, Copy, Debug)]
pub(crate) struct SeparatorBand {
    /// Lowest fraction a gesture may write.
    pub(crate) min: f32,
    /// Highest fraction a gesture may write. Always `1.0 - min`, so always `>= min`.
    pub(crate) max: f32,
    /// The stored fraction as this frame's geometry can honour it: `fraction.clamp(min, max)`.
    pub(crate) effective: f32,
}

impl SeparatorBand {
    /// The band for a boundary that has the whole row to itself: `range` is the node's extent
    /// along the split axis, `extra` is
    /// [`SeparatorStyle::extra`](crate::SeparatorStyle::extra); both in points.
    ///
    /// What every boundary had until a row could hold three, and what **drawing** still uses:
    /// each boundary is pushed into the row's own margins on its own, and the row's boundaries
    /// being monotone (running sums of non-negative weights) keeps their order. See
    /// [`between`](Self::between) for the gesture's band, which is narrower.
    pub(crate) fn new(fraction: f32, range: f32, extra: f32) -> Self {
        Self::between(fraction, 0.0, 1.0, range, extra)
    }

    /// The band for a boundary hemmed in by its **neighbours**: `lo` and `hi` are the boundaries
    /// on either side of it, `0.0` and `1.0` at the ends of the row.
    ///
    /// A gesture writes through this one, and on a row of three that is not a refinement but the
    /// difference between a valid tree and an invalid one. `RowNode::set_boundary` gives child
    /// `k` the room between `lo` and where the boundary lands and child `k + 1` the room up to
    /// `hi`; a boundary written past either neighbour therefore leaves one of them a **negative
    /// weight**, which is `TreeViolation::RowShareNegative`. On a pair the two neighbours are the
    /// row's own ends, so this could not arise and the whole row was the right band — which is
    /// why it was the only one for six stages.
    ///
    /// Found by the DST sweep at stage 7 rather than by reading: `DragSeparator` pulled gap 0 of
    /// a row of three past gap 1 and the oracle reported the violation.
    pub(crate) fn between(fraction: f32, lo: f32, hi: f32, range: f32, extra: f32) -> Self {
        // A node with no extent has no room for a margin and nothing to show either. Answering
        // "no constraint" keeps this a total function of its arguments instead of a special
        // case every caller has to remember; the callers that could act on it guard on `range`
        // anyway, because `delta / range` is not finite here.
        //
        // The negation is load-bearing and clippy's rewrite of it is not equivalent: `range`
        // is `f32`, so a NaN — which is what a degenerate rectangle hands us — answers `false`
        // to *every* comparison. `!(range > 0.0)` is therefore true for NaN and takes this
        // early return, while the suggested `range <= 0.0` is false for it and would carry the
        // NaN into the arithmetic below, where it silently becomes a fraction.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(range > 0.0) {
            return Self {
                min: lo,
                max: hi,
                effective: fraction,
            };
        }

        // Capping the margin at half the *room between the neighbours* is what makes an
        // impossible margin degrade sensibly: the band shrinks to a point and the boundary sits
        // at the centre of that room — an equal split of what there is, which is the least-bad
        // answer when there is none to give. The previous normalisation `(min.min(max),
        // max.max(min))` instead *swapped* the inverted pair, so `extra / range >= 1` produced
        // the interval `(0, 1)`: no constraint at all, exactly on the nodes where it was the
        // only thing standing between a child and zero size. Found by the frame harness — a drag
        // on a 175 px node drove `fraction` to 0.0.
        //
        // `room` is the whole row for a pair, so this is the same arithmetic it always was.
        //
        // The capped case is written as *one* point rather than as two ends that happen to meet,
        // and that is not tidiness: `lo + room/2` and `hi - room/2` are the same number in
        // arithmetic and need not be in `f32`. The sweep found them 3e-8 apart the wrong way
        // round (`min = 0.26666668`, `max = 0.26666665`) on a squeezed window, and
        // `f32::clamp` **panics** when its min exceeds its max — so the crate went down inside a
        // junction drag, on a row whose two neighbours had been squeezed to the margin.
        let room = (hi - lo).max(0.0);
        let margin = extra / range;
        let half = room * 0.5;
        let (min, max) = if margin >= half {
            let centre = lo + half;
            (centre, centre)
        } else {
            (lo + margin, hi - margin)
        };
        Self {
            min,
            max,
            effective: fraction.clamp(min, max),
        }
    }

    /// Where the boundary falls along the split axis, given the node's near edge and its extent
    /// along that axis — the same `range` this band was built from.
    ///
    /// A node with no extent has no boundary: it is its own edge. That case is here rather than
    /// at the call sites because it is the only place both of them would have had to remember
    /// it, and one of them already got it wrong by omission once.
    pub(crate) fn midpoint(&self, min: f32, range: f32) -> f32 {
        if range > 0.0 {
            min + range * self.effective
        } else {
            min
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Extent, Run, SeparatorBand, cut_runs};

    // ---------------------------------------------------------------------------------------
    // `cut_runs`, in one dimension. Stated on the arithmetic, where the property lives: a pair
    // never has a middle of two, a fixed child among open ones, or a trailing run beside a
    // leading one, so nothing on screen can reach these until stage 7 — and the corpus probes,
    // which judge parity on pairs, cannot tell a right n-ary cut from a wrong one.
    // ---------------------------------------------------------------------------------------

    fn whole(at: f32) -> f32 {
        at.round()
    }

    /// Fixed children at both ends stack from their own edges; the open ones between them share
    /// what the runs leave, and the one divider is between the two open neighbours.
    #[test]
    fn a_leading_run_a_middle_and_a_trailing_run() {
        let extents = [
            Extent::Fixed(10.0),
            Extent::Weighted(1.0),
            Extent::Weighted(1.0),
            Extent::Fixed(10.0),
        ];
        let cut = cut_runs(0.0, 100.0, &extents, 2.0, whole, whole, false);
        assert_eq!(
            cut.spans,
            vec![
                (None, Some(10.0)),
                (Some(12.0), Some(49.0)),
                (Some(51.0), Some(88.0)),
                (Some(90.0), None),
            ]
        );
        assert_eq!(cut.dividers, vec![None, Some((49.0, 51.0)), None]);
        assert_eq!(
            cut.runs,
            vec![Run::Leading, Run::Middle, Run::Middle, Run::Trailing]
        );
    }

    /// With nothing open the whole row is a leading run, and whether the last child reaches the
    /// far edge is the caller's decision — the vertical rule says yes, the horizontal one (Стас,
    /// 30.08: strips for everyone, the rest empty) says no.
    #[test]
    fn with_nothing_open_the_last_fixed_child_keeps_the_rest_only_if_asked() {
        let extents = [Extent::Fixed(10.0); 3];
        let keeps = cut_runs(0.0, 100.0, &extents, 2.0, whole, whole, true);
        assert_eq!(
            keeps.spans,
            vec![
                (None, Some(10.0)),
                (Some(12.0), Some(22.0)),
                (Some(24.0), None)
            ]
        );
        let leaves = cut_runs(0.0, 100.0, &extents, 2.0, whole, whole, false);
        assert_eq!(
            leaves.spans,
            vec![
                (None, Some(10.0)),
                (Some(12.0), Some(22.0)),
                (Some(24.0), Some(34.0)),
            ]
        );
        for cut in [&keeps, &leaves] {
            assert_eq!(cut.runs, vec![Run::Leading; 3]);
            assert_eq!(cut.dividers, vec![None, None]);
        }
    }

    /// A fixed child between two open ones keeps exactly its length, the open ones split the
    /// rest, and **both** gaps beside it draw a line: the two open children have only a strip
    /// between them, so each of the strip's edges is a handle on the one boundary they share.
    ///
    /// The lines are at the strip's edges and not at a ratio — which is why the file used to say
    /// there were none at all. That left the two open children with no line between them
    /// anywhere and no way to be resized against each other, which is the defect this pins.
    #[test]
    fn a_fixed_child_among_open_ones_is_grabbed_by_both_its_edges() {
        let extents = [
            Extent::Weighted(1.0),
            Extent::Fixed(10.0),
            Extent::Weighted(1.0),
        ];
        let cut = cut_runs(0.0, 100.0, &extents, 2.0, whole, whole, false);
        assert_eq!(
            cut.spans,
            vec![
                (None, Some(43.0)),
                (Some(45.0), Some(55.0)),
                (Some(57.0), None)
            ]
        );
        assert_eq!(
            cut.dividers,
            vec![Some((43.0, 45.0)), Some((55.0, 57.0))],
            "one line where the near column ends, one where the far one begins"
        );
        assert_eq!(cut.runs, vec![Run::Middle; 3]);
    }

    /// A strip at the row's **end** still draws no line beside it — there is only one open child
    /// there, and nothing for it to trade with. The positive control for the test above: without
    /// it, "a strip has handles" would pass just as well if every gap beside a strip grew one.
    #[test]
    fn a_fixed_child_at_the_end_of_the_row_has_nothing_to_trade_with() {
        let leading = [
            Extent::Fixed(10.0),
            Extent::Weighted(1.0),
            Extent::Weighted(1.0),
        ];
        let cut = cut_runs(0.0, 100.0, &leading, 2.0, whole, whole, false);
        assert_eq!(
            cut.dividers[0], None,
            "no line between the strip and the column beside it"
        );
        assert!(
            cut.dividers[1].is_some(),
            "control: the two open columns still have theirs"
        );

        let trailing = [
            Extent::Weighted(1.0),
            Extent::Weighted(1.0),
            Extent::Fixed(10.0),
        ];
        let cut = cut_runs(0.0, 100.0, &trailing, 2.0, whole, whole, false);
        assert!(cut.dividers[0].is_some(), "control: the same, mirrored");
        assert_eq!(cut.dividers[1], None);
    }

    /// Open children whose weights add up to nothing are not a proportion of anything; they
    /// share equally rather than divide by zero.
    #[test]
    fn open_children_with_no_weight_between_them_share_equally() {
        let extents = [
            Extent::Fixed(10.0),
            Extent::Weighted(0.0),
            Extent::Weighted(0.0),
        ];
        let cut = cut_runs(0.0, 100.0, &extents, 2.0, whole, whole, false);
        assert_eq!(
            cut.spans,
            vec![
                (None, Some(10.0)),
                (Some(12.0), Some(55.0)),
                (Some(57.0), None)
            ]
        );
        assert_eq!(cut.dividers, vec![None, Some((55.0, 57.0))]);
    }

    /// **The two axes snap their runs differently, and it is inherited, not chosen.** The
    /// horizontal branch snaps the run itself (`right_start = cut_at(left_end + separator)`
    /// with `left_end` already snapped); the vertical one snaps each edge from the unsnapped
    /// run. At `pixels_per_point = 1` the two agree, which is why the corpus probes and the
    /// sweep cannot tell them apart; at a fractional scale they land a pixel apart. A parity
    /// stage keeps each branch its own scheme, and this pins that it did: change one to the
    /// other and a strip's divider moves a pixel on every HiDPI screen. Unifying them is a
    /// decision about pixels, not a cleanup — see `cut_runs`.
    #[test]
    fn the_two_axes_snap_their_runs_differently_and_it_is_inherited() {
        const PIXELS_PER_POINT: f32 = 1.5;
        let snap = |at: f32| (at * PIXELS_PER_POINT).round() / PIXELS_PER_POINT;
        let pixels = |at: f32| (at * PIXELS_PER_POINT).round();
        // Two fixed children, not one: the *second* strip's far edge is what tells "the run is
        // snapped" from "only the cut is" — with one strip the two are the same number.
        let extents = [
            Extent::Fixed(24.4),
            Extent::Fixed(24.4),
            Extent::Weighted(1.0),
        ];

        let vertical = cut_runs(0.0, 100.0, &extents, 1.2, snap, |at| at, true);
        let horizontal = cut_runs(0.0, 100.0, &extents, 1.2, snap, snap, false);

        // The first edge is the same number either way: it is snapped from the row's edge.
        assert_eq!(pixels(vertical.spans[0].1.unwrap()), 37.0);
        assert_eq!(pixels(horizontal.spans[0].1.unwrap()), 37.0);
        // The next one is not: 24.4 + 1.2 = 25.6 → pixel 38 from the unsnapped run, but
        // 24.667 (pixel 37) + 1.2 = 25.867 → pixel 39 from the snapped one.
        assert_eq!(
            pixels(vertical.spans[1].0.unwrap()),
            38.0,
            "the vertical branch snaps each edge from the run in points"
        );
        assert_eq!(
            pixels(horizontal.spans[1].0.unwrap()),
            39.0,
            "the horizontal branch snaps the run itself at every step"
        );
        // And the second strip's far edge carries the difference on: 25.6 + 24.4 = 50 → pixel
        // 75 in points, against 26 (pixel 39) + 24.4 = 50.4 → pixel 76 along the snapped run.
        assert_eq!(pixels(vertical.spans[1].1.unwrap()), 75.0);
        assert_eq!(
            pixels(horizontal.spans[1].1.unwrap()),
            76.0,
            "the horizontal run is carried snapped, so the next strip starts from a pixel"
        );
    }

    /// Open children between the runs split what the fixed ones leave **by weight**, not
    /// equally: with weights 1 and 3 the second takes three times the first.
    #[test]
    fn open_children_split_what_the_fixed_ones_leave_by_weight() {
        let extents = [
            Extent::Fixed(10.0),
            Extent::Weighted(1.0),
            Extent::Weighted(3.0),
        ];
        // 102 − 12 − 2 = 88 to share: 22 and 66.
        let cut = cut_runs(0.0, 102.0, &extents, 2.0, whole, whole, false);
        assert_eq!(
            cut.spans,
            vec![
                (None, Some(10.0)),
                (Some(12.0), Some(34.0)),
                (Some(36.0), None)
            ]
        );
        assert_eq!(cut.dividers, vec![None, Some((34.0, 36.0))]);
    }

    // ---------------------------------------------------------------------------------------
    // `SeparatorBand`: the limits, and the fraction as the geometry can honour it.
    // ---------------------------------------------------------------------------------------

    /// A degenerate node hands the band a `range` that is not a positive number, and the two
    /// ways it can fail to be one are **not** the same test: zero is ordinary arithmetic, NaN
    /// answers `false` to every comparison it is put in.
    ///
    /// This pins the `#[allow(clippy::neg_cmp_op_on_partial_ord)]` above `SeparatorBand::new`.
    /// The lint's suggested rewrite — `range <= 0.0` — is false for NaN, so the guard would be
    /// skipped and the NaN would flow into `fraction.clamp(min, max)` and out as a fraction the
    /// tree then stores. Taking the suggestion turns the second half of this test red, which is
    /// the whole reason the `allow` is allowed to stay.
    #[test]
    fn a_range_that_is_not_a_positive_number_constrains_nothing() {
        for range in [0.0, -1.0, f32::NAN] {
            let band = SeparatorBand::new(0.25, range, 4.0);
            assert_eq!(
                (band.min, band.max, band.effective),
                (0.0, 1.0, 0.25),
                "range {range} should have left the fraction alone"
            );
        }
    }

    /// The band is symmetric by construction, and the fraction is what the geometry can honour.
    #[test]
    fn a_margin_too_big_for_the_node_collapses_the_band_to_the_centre() {
        let band = SeparatorBand::new(0.9, 10.0, 40.0);
        assert_eq!(band.min, band.max, "an impossible margin leaves no band");
        assert_eq!(
            band.effective, band.min,
            "the boundary sits where the band is"
        );
        assert!(
            (band.effective - 0.5).abs() < f32::EPSILON,
            "the least-bad answer with no room to give is an equal split, got {}",
            band.effective
        );
    }

    /// **A band squeezed between two neighbours collapses to a point, and never past it.**
    ///
    /// `lo + room/2` and `hi - room/2` are one number in arithmetic and two in `f32`, and the
    /// order they come out in is not fixed: the sweep found them 3e-8 apart the wrong way round
    /// on a squeezed window, which made `f32::clamp` panic (`min > max`) inside a junction drag —
    /// the crate going down, not a boundary going astray. Fixed by writing the capped case as one
    /// point instead of two ends that ought to meet, and pinned here over a spread of positions
    /// and rooms rather than at the one triple that happened to fail.
    #[test]
    fn a_band_with_no_room_between_its_neighbours_is_a_single_point() {
        for lo in [0.0, 0.1, 0.26666668, 1.0 / 3.0, 0.7] {
            for room in [0.0, 1e-6, 0.05, 0.2] {
                let hi = lo + room;
                // A margin far larger than half the room, which is the regime the cap is for.
                let band = SeparatorBand::between(0.5 * (lo + hi), lo, hi, 100.0, 175.0);
                assert!(
                    band.min <= band.max,
                    "lo {lo}, room {room}: band came out inverted ({}, {})",
                    band.min,
                    band.max
                );
                assert_eq!(band.min, band.max, "lo {lo}, room {room}: not a point");
                // The clamp the callers run, which is what panicked.
                let _ = 0.42_f32.clamp(band.min, band.max);
            }
        }
    }
}
