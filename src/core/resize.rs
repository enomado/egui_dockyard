//! How a boundary drag divides the room it takes: the arithmetic, with no opinion about who
//! asked for it.
//!
//! A row divides its length between children by weight. Moving one boundary is therefore a
//! question of *bookkeeping* — which weights give up room, which take it — and there is more than
//! one defensible answer:
//!
//! | [`SepBehavior`] | Who pays |
//! |---|---|
//! | [`Chain`](SepBehavior::Chain) | the near neighbour, then the one behind it once the near one is at its minimum, to the end of the row |
//! | [`Pair`](SepBehavior::Pair) | exactly the two children the gap lies between |
//! | [`Proportional`](SepBehavior::Proportional) | every child: one side gives up room in proportion to its weight, the other takes it the same way |
//! | [`Frame`](SepBehavior::Frame) | one framing child against a block that keeps its internal proportions |
//!
//! **This module chooses none of them.** Which behaviour a given separator has is policy, and
//! policy belongs to whoever draws the screen: this crate maps modifiers to a behaviour in
//! `show_divider`, and the application's grid screens have their own rule per column edge
//! (`welllog::grid_render::resolve_behavior`). Keeping the two apart is the reason this is a
//! module of free functions over `&mut [f32]` rather than a method on a row.
//!
//! # Why it lives here
//!
//! It arrived from the application, where `ss_grid_layout::separators` had held it since 14.08 —
//! itself written after the depth screen was found carrying a *simplified copy* of the same
//! arithmetic, and the simplification was exactly what the user noticed (the copy handled one
//! axis, so one header would not drag). One copy, one home. The home is here because this crate
//! is the one that ships, and the application already depends on it.
//!
//! # Units
//!
//! Weights are relative and unnormalised — `[1.0, 1.0]` and `[0.5, 0.5]` are the same picture,
//! as [`Share`](crate::core::tree::Share) explains. A drag, though, arrives in **points**, and a
//! minimum size is in points too. The bridge is `size_in_point`: the caller says how long each
//! child currently is, and the ratio of the two totals converts one into the other. So this
//! module needs no rectangle, no scale factor and no style — which is what lets it sit in the
//! egui-free core.
//!
//! `min_size` is a **parameter and not a constant**, because the two callers genuinely differ:
//! this crate's minimum is [`SeparatorStyle::extra`](crate::SeparatorStyle::extra), the
//! application's grids use their own `MIN_SIZE`. A constant here would have made those one
//! number by accident.

/// How a separator drag redistributes the row's weights. See the module docs for the table.
///
/// Cloned rather than copied because [`Frame`](Self::Frame) carries the block it trades against;
/// the other three are trivially copyable but a mixed enum is not worth the special case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SepBehavior {
    /// One neighbour grows, and the chain on the other side shrinks **greedily**: the near
    /// neighbour down to `min_size`, then the next one, and so on to the end of the row.
    ///
    /// The line keeps following the cursor instead of stopping at the first child that ran out,
    /// which is what "the panels push each other along" means to a hand.
    Chain,

    /// The classic splitter: exactly the two children beside the gap trade, everything else in
    /// the row stands still.
    Pair,

    /// A framing child (`frame_idx`) trades against `block`, which grows and shrinks
    /// **proportionally** so its internal boundaries keep their ratios.
    ///
    /// The application's grid edges — the depth column, the notes column — are this. No gesture
    /// in this crate selects it; it is here because it is one arm of the same function, and
    /// deleting it would fork this file from the policy that uses it on day one.
    Frame {
        /// Indices of the children that make up the block, in row order.
        block: Vec<usize>,
        /// Index of the framing child itself.
        frame_idx: usize,
    },

    /// Every weight moves: the side the boundary travels into gives up room in proportion to
    /// what each child has, and the other side takes it the same way.
    ///
    /// Unlike [`Chain`](Self::Chain), no child has to reach its minimum first — the far side of
    /// the row moves from the first pixel.
    Proportional,
}

/// Moves the boundary between children `gap` and `gap + 1` by `delta` **points**, rewriting the
/// row's weights in place.
///
/// `delta` is signed: negative moves the boundary towards the start of the row. `size_in_point(k)`
/// is child `k`'s current length in points, and `min_size` the length no child may be pushed
/// below.
///
/// Total weight is preserved by every behaviour — room is moved, never created or destroyed —
/// which is what keeps the row's layout stable under a drag that hits a limit.
///
/// # Panics
///
/// If `gap` is not a gap of `shares` (that is, `gap + 1 >= shares.len()`). A caller holds a gap
/// it took from this very row, so an out-of-range one is the caller's bug rather than a case to
/// answer.
#[track_caller]
pub fn apply_drag(
    behavior: &SepBehavior,
    shares: &mut [f32],
    gap: usize,
    delta: f32,
    min_size: f32,
    size_in_point: impl Fn(usize) -> f32,
) {
    let num = shares.len();
    assert!(
        gap + 1 < num,
        "boundary {gap} was dragged in a row of {num} children"
    );
    let (left, right) = (gap, gap + 1);

    match behavior {
        SepBehavior::Chain => {
            if delta < 0.0 {
                // Grow the right child, shrink what is to the left, near to far: a child that
                // has reached `min_size` does not stop the drag, it is simply passed over and
                // the next one pays.
                let children: Vec<usize> = (0..=gap).rev().collect();
                shares[right] +=
                    shrink_shares(shares, &children, delta.abs(), min_size, &size_in_point);
            } else {
                let children: Vec<usize> = (gap + 1..num).collect();
                shares[left] +=
                    shrink_shares(shares, &children, delta.abs(), min_size, &size_in_point);
            }
        }
        SepBehavior::Pair => {
            if delta < 0.0 {
                shares[right] +=
                    shrink_shares(shares, &[left], delta.abs(), min_size, &size_in_point);
            } else {
                shares[left] +=
                    shrink_shares(shares, &[right], delta.abs(), min_size, &size_in_point);
            }
        }
        SepBehavior::Frame { block, frame_idx } => {
            // Does the framing child grow under this drag? A frame on the left grows rightwards,
            // one on the right grows leftwards.
            let frame_grows = if *frame_idx == left {
                delta > 0.0
            } else {
                delta < 0.0
            };
            if frame_grows {
                let lost = shrink_shares_proportional(
                    shares,
                    block,
                    delta.abs(),
                    min_size,
                    &size_in_point,
                );
                shares[*frame_idx] += lost;
            } else {
                let freed =
                    shrink_shares(shares, &[*frame_idx], delta.abs(), min_size, &size_in_point);
                grow_shares_proportional(shares, block, freed);
            }
        }
        SepBehavior::Proportional => {
            let lhs: Vec<usize> = (0..=left).collect();
            let rhs: Vec<usize> = (right..num).collect();
            let (shrink, grow) = if delta < 0.0 { (lhs, rhs) } else { (rhs, lhs) };
            let freed =
                shrink_shares_proportional(shares, &shrink, delta.abs(), min_size, &size_in_point);
            grow_shares_proportional(shares, &grow, freed);
        }
    }
}

/// Grows a group by `add_shares` in total, split **proportionally** to what each member already
/// has, so the group's internal boundaries do not move.
///
/// `min_size` plays no part: growing never violates a minimum. A group whose weights are all zero
/// is grown equally — the only answer that does not divide by nothing.
pub fn grow_shares_proportional(shares: &mut [f32], group: &[usize], add_shares: f32) {
    if add_shares <= 0.0 || group.is_empty() {
        return;
    }
    let total: f32 = group.iter().map(|&c| shares[c]).sum();
    if total <= 0.0 {
        let each = add_shares / group.len() as f32;
        for &c in group {
            shares[c] += each;
        }
        return;
    }
    for &c in group {
        shares[c] += add_shares * shares[c] / total;
    }
}

/// Shrinks a group by `target_in_points` in total, taken from each member **in proportion** to
/// what it has and never below `min_size`. Answers the weight actually freed.
///
/// The proportional counterpart of [`shrink_shares`], which takes greedily instead. The request
/// is clamped by the group's total room above the minimum up front, so a group asked for more
/// than it has gives what it has rather than distributing an impossible target and rounding.
pub fn shrink_shares_proportional(
    shares: &mut [f32],
    group: &[usize],
    target_in_points: f32,
    min_size: f32,
    size_in_point: impl Fn(usize) -> f32,
) -> f32 {
    if group.is_empty() {
        return 0.0;
    }
    let mut total_shares = 0.0;
    let mut total_points = 0.0;
    for &c in group {
        total_shares += shares[c];
        total_points += size_in_point(c);
    }
    if total_points <= 0.0 {
        return 0.0;
    }
    let shares_per_point = total_shares / total_points;
    let min_size_in_shares = shares_per_point * min_size;
    let spare_total = (total_shares - min_size_in_shares * group.len() as f32).max(0.0);
    let target_in_shares = (shares_per_point * target_in_points).min(spare_total);
    if target_in_shares <= 0.0 {
        return 0.0;
    }
    let mut total_lost = 0.0;
    for &c in group {
        let want = target_in_shares * shares[c] / total_shares;
        let spare = (shares[c] - min_size_in_shares).max(0.0);
        let take = want.min(spare);
        shares[c] -= take;
        total_lost += take;
    }
    total_lost
}

/// Shrinks the listed children by `target_in_points` in total, **greedily** in the order given
/// and never below `min_size`. Answers the weight actually freed.
///
/// The order is the caller's statement of near-to-far: each child gives everything it can spare
/// before the next is asked, which is what makes a child that has run out pass the drag along
/// instead of stopping it.
pub fn shrink_shares(
    shares: &mut [f32],
    children: &[usize],
    target_in_points: f32,
    min_size: f32,
    size_in_point: impl Fn(usize) -> f32,
) -> f32 {
    if children.is_empty() {
        return 0.0;
    }
    let mut total_shares = 0.0;
    let mut total_points = 0.0;
    for &c in children {
        total_shares += shares[c];
        total_points += size_in_point(c);
    }
    if total_points <= 0.0 {
        return 0.0;
    }
    let shares_per_point = total_shares / total_points;
    let min_size_in_shares = shares_per_point * min_size;
    let target_in_shares = shares_per_point * target_in_points;
    let mut total_shares_lost = 0.0;

    for &c in children {
        let share = &mut shares[c];
        let spare_share = (*share - min_size_in_shares).max(0.0);
        let shares_needed = (target_in_shares - total_shares_lost).max(0.0);
        let shrink_by = f32::min(spare_share, shares_needed);
        *share -= shrink_by;
        total_shares_lost += shrink_by;
    }
    total_shares_lost
}

#[cfg(test)]
mod tests {
    //! The arithmetic on its own, with no dock and no screen. Ported with the code from
    //! `ss_grid_layout::separators`, which is where the first three came from.

    use super::*;

    const NUM: usize = 4;
    /// Every child 100 points long, so one unit of weight is 100 points and the expectations
    /// below can be read as pixels.
    const CELL_PT: f32 = 100.0;
    const MIN: f32 = 32.0;

    fn equal_row() -> Vec<f32> {
        vec![1.0_f32; NUM]
    }

    /// The chain: pulling boundary `1|2` left by 80 points asks child 1 for more than the 68 it
    /// can spare, and the remaining 12 come off child 0 — "1 moves and pushes 0".
    #[test]
    fn a_chain_shrinks_past_the_child_that_ran_out() {
        let mut shares = equal_row();
        let delta = -80.0_f32;
        let spare1 = CELL_PT - MIN; // 68 points = 0.68 of a weight
        let overflow = -delta - spare1; // 12 points

        apply_drag(&SepBehavior::Chain, &mut shares, 1, delta, MIN, |_| CELL_PT);

        let expected = [
            1.0 - overflow / CELL_PT,
            MIN / CELL_PT,
            1.0 + (-delta) / CELL_PT,
            1.0,
        ];
        for (i, (a, e)) in shares.iter().zip(expected).enumerate() {
            assert!((a - e).abs() < 1e-4, "weight {i}: {a} != {e} ({shares:?})");
        }
    }

    /// The pair moves exactly two children — the difference from the chain on the same scene.
    #[test]
    fn a_pair_moves_only_its_two_neighbours() {
        let mut shares = equal_row();
        apply_drag(&SepBehavior::Pair, &mut shares, 1, 30.0, MIN, |_| CELL_PT);
        assert!((shares[0] - 1.0).abs() < 1e-4, "the far left stood still");
        assert!((shares[3] - 1.0).abs() < 1e-4, "the far right stood still");
        assert!(
            (shares[1] + shares[2] - 2.0).abs() < 1e-4,
            "the pair traded within itself"
        );
    }

    /// Every behaviour conserves the total: room is moved, never made or lost.
    #[test]
    fn every_drag_preserves_the_total_weight() {
        for behavior in [
            SepBehavior::Chain,
            SepBehavior::Pair,
            SepBehavior::Proportional,
            SepBehavior::Frame {
                block: vec![1, 2, 3],
                frame_idx: 0,
            },
        ] {
            let mut shares = equal_row();
            let before: f32 = shares.iter().sum();
            apply_drag(&behavior, &mut shares, 1, -50.0, MIN, |_| CELL_PT);
            let after: f32 = shares.iter().sum();
            assert!((before - after).abs() < 1e-4, "{behavior:?}: {shares:?}");
        }
    }

    /// The proportional mode moves the far side of the row **from the first point**, without any
    /// child having to reach its minimum first. That is what distinguishes it from the chain,
    /// which on this same soft drag leaves child 3 exactly where it was.
    #[test]
    fn proportional_moves_the_far_side_at_once() {
        let soft = 30.0_f32;

        let mut chained = equal_row();
        apply_drag(&SepBehavior::Chain, &mut chained, 1, soft, MIN, |_| CELL_PT);
        assert!(
            (chained[3] - 1.0).abs() < 1e-4,
            "a chain this soft never reaches child 3: {chained:?}"
        );

        let mut spread = equal_row();
        apply_drag(
            &SepBehavior::Proportional,
            &mut spread,
            1,
            soft,
            MIN,
            |_| CELL_PT,
        );
        assert!(
            1.0 - spread[3] > 1e-3,
            "proportional asks child 3 straight away: {spread:?}"
        );
        // Both children on the shrinking side had the same weight, so they gave the same room.
        assert!(
            (spread[2] - spread[3]).abs() < 1e-4,
            "equal children pay equally: {spread:?}"
        );
    }

    /// Weights that are *not* equal are what tells "proportional" from "in equal parts": a child
    /// holding three times as much gives up three times as much.
    ///
    /// Equal weights hide the difference — the same trap stage 4 of the n-ary plan recorded when
    /// a `1:1` scene could not tell `free/count` from `free·w/Σw`.
    #[test]
    fn proportional_takes_in_proportion_and_not_in_equal_parts() {
        // Children 2 and 3 are the shrinking side, and 3 has three times the room.
        let mut shares = vec![1.0, 1.0, 1.0, 3.0];
        let sizes = shares.clone();
        apply_drag(&SepBehavior::Proportional, &mut shares, 1, 40.0, MIN, |i| {
            sizes[i] * CELL_PT
        });

        let paid_by_2 = 1.0 - shares[2];
        let paid_by_3 = 3.0 - shares[3];
        assert!(paid_by_2 > 1e-4, "both paid something: {shares:?}");
        assert!(
            (paid_by_3 / paid_by_2 - 3.0).abs() < 1e-2,
            "the bigger child paid three times as much: {paid_by_2} vs {paid_by_3} ({shares:?})"
        );
    }

    /// A group asked for more than it has gives what it has and says so, rather than reporting a
    /// freed amount nobody actually gave up — which would appear on the other side of the trade
    /// as room from nowhere.
    #[test]
    fn a_group_with_nothing_left_frees_nothing() {
        let mut shares = vec![MIN / CELL_PT; NUM];
        let freed = shrink_shares_proportional(&mut shares, &[2, 3], 500.0, MIN, |_| MIN);
        assert!(
            freed.abs() < 1e-6,
            "nothing to give, nothing freed: {freed}"
        );
        assert!(
            shares.iter().all(|&s| (s - MIN / CELL_PT).abs() < 1e-6),
            "and nobody was pushed below the minimum: {shares:?}"
        );
    }

    /// The framing child trades against a block that keeps its own proportions: the block's
    /// internal boundary does not move, which is the whole point of the mode.
    #[test]
    fn a_frame_keeps_the_blocks_internal_proportions() {
        let frame = SepBehavior::Frame {
            block: vec![1, 2, 3],
            frame_idx: 0,
        };
        let mut shares = vec![1.0, 1.0, 2.0, 1.0];
        let ratio_before = shares[2] / shares[1];
        let sizes = shares.clone();

        apply_drag(&frame, &mut shares, 0, 40.0, MIN, |i| sizes[i] * CELL_PT);

        assert!(shares[0] > 1.0, "the frame grew: {shares:?}");
        assert!(
            (shares[2] / shares[1] - ratio_before).abs() < 1e-3,
            "the block shrank without moving its own boundary: {shares:?}"
        );
    }
}
