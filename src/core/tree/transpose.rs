//! Transposing the grouping around a crossing: the one edit that changes *how* leaves are
//! nested without changing where any of them is on screen.
//!
//! # What it is
//!
//! Two chains of splits lie side by side, separated by one split — call it `outer`. Somewhere
//! along them a divider of the first lines up with a divider of the second, and the two read as
//! one line crossing `outer`'s. There are two ways to group the same picture:
//!
//! ```text
//!   outer groups two bands           outer is cut by the crossing line
//!   ┌─────┬─────┐                    ┌─────┬─────┐
//!   │  A  │  C  │                    │  A  │  C  │
//!   ├─────┼─────┤        ⇄           ├─────┴─────┤
//!   │  B  │  D  │                    │  B  │  D  │
//!   └─────┴─────┘                    └─────┴─────┘
//!    (A,B) and (C,D)                  (A,C) and (B,D)
//! ```
//!
//! Both draw the same four rectangles; they differ in what a drag of the middle divider does.
//! Transposing swaps one reading for the other.
//!
//! # Why the geometry is an argument and not a component
//!
//! Everything about *which node ends up where* is read out of the tree here — that is what
//! [`Tree::chain`] is for. What cannot be read out of the tree is where the boundaries are: a
//! fraction is a share of a rectangle, and the rectangles live in the layout pass. Since the
//! promise of a transposition is that **no pixel moves**, the boundaries the picture already has
//! are an input to it, handed in as `bounds`, and every fraction written below is derived from
//! them rather than invented.
//!
//! Passing measured *boundaries* rather than measured *sizes* is deliberate: the boundary of a
//! split is exactly what its fraction names, while a part's size has half a separator width
//! folded into each inner side, and reconstructing a ratio from sizes drifts it — most visibly
//! on small tiles.

use crate::core::tree::regroup::Regroup;
use crate::core::tree::{NodeId, RowGap, Share, Tree};

/// A chain of same-oriented rows, flattened in screen order.
///
/// `dividers[k]` is the gap whose boundary falls between `parts[k]` and `parts[k + 1]`, so
/// there is always exactly one fewer divider than there are parts. A divider is a *gap of a
/// row* and not the row itself: while rows are pairs the two coincide, and the moment a row
/// holds three parts it contributes two dividers and is one node. The rows the dividers belong
/// to double as the pool of ids a transposition rebuilds the chain out of — a chain taken apart
/// and re-nested needs exactly as many splits as it had, and reusing its own keeps every id, tab
/// and focus flag below it untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chain {
    /// The subtrees hanging off the chain, in screen order (left to right, or top to bottom).
    pub(crate) parts: Vec<NodeId>,

    /// The `parts.len() - 1` gaps between them, in the same order.
    pub(crate) dividers: Vec<RowGap>,
}

impl<Tab> Tree<Tab> {
    /// Flatten the chain of `horizontal`-oriented splits rooted at `root`.
    ///
    /// A node of the other orientation — or a leaf — is a *part*: the walk stops there and does
    /// not look inside it. So a chain is "everything the eye reads as one row (or one column)",
    /// however deeply the splits making it up happen to be nested.
    ///
    /// The walk is in-order, which is what puts both lists in screen order: everything the first
    /// child contributes lies before the split's own boundary, and everything the second
    /// contributes lies after it, at every level.
    pub(crate) fn chain(&self, root: NodeId, horizontal: bool) -> Chain {
        let mut chain = Chain {
            parts: Vec::new(),
            dividers: Vec::new(),
        };
        self.collect_chain(root, horizontal, &mut chain);
        chain
    }

    fn collect_chain(&self, node: NodeId, horizontal: bool, chain: &mut Chain) {
        let in_chain = if horizontal {
            self[node].is_horizontal()
        } else {
            self[node].is_vertical()
        };
        if !in_chain {
            chain.parts.push(node);
            return;
        }
        // A loop, now that a divider has an address of its own: between every two children of
        // the row lies one of its gaps, and that gap — not the row — is what the divider list
        // names. This used to be a pair, because "the divider of this row" named the row, and a
        // loop would have pushed one id twice and called it two dividers.
        let row = self[node].get_row().expect("a node in a chain is a row");
        for (index, child) in row.children().iter().copied().enumerate() {
            if index > 0 {
                chain.dividers.push(RowGap {
                    row: node,
                    gap: crate::core::tree::GapIndex(index - 1),
                });
            }
            self.collect_chain(child, horizontal, chain);
        }
    }

    /// Regroup around the crossing at `at`, keeping every leaf exactly where it is on screen.
    ///
    /// `outer` is the **gap** the two chains lie on either side of, and `at` names the divider of
    /// each that the crossing is made of: `at[0]` counts along the first chain, `at[1]` along the
    /// second. The line through that crossing runs the full extent of both chains, so it can
    /// carry the whole of what lies between them: afterwards those two neighbours are replaced by
    /// **one** node, cut by that line into two halves, and each half holds what the two chains had
    /// on its side of it.
    ///
    /// A gap and not a row, since stage 7: a row of three has two gaps and a crossing sits on
    /// exactly one of them, between exactly two of the children. On a row of two the two readings
    /// coincide — the row's only gap is the row — and the replacement leaves the row with a single
    /// child, so the row *is* that node, which is how this used to be written. The rest of the
    /// row's children are not part of the gesture and are not touched.
    ///
    /// The 2x2 case — one divider per chain, both chains two parts — is this with every
    /// sub-chain of length one, and comes out with the gap's two old neighbours still playing the
    /// two halves. The general case has `n + m` rectangles and no "swap" reading, but the promise
    /// is the same: nothing moves except the two dividers that were out of line, which become one
    /// line — their average — so each moves by half of whatever gap the caller's alignment
    /// tolerance let through.
    ///
    /// `bounds[c]` are the `parts + 1` boundaries of chain `c` along its own axis, ascending:
    /// its two outer edges with each divider's position in between. `stack_fraction` is the one
    /// number from the *other* axis — where the gap's own boundary sits between the **two
    /// neighbours'** outer edges — because that is the share each rebuilt half keeps for the
    /// first chain.
    ///
    /// # Panics
    ///
    /// If `outer` does not name a gap of a live row, if `at` does not name a divider of both
    /// chains, or if `bounds` does not describe them. All are caller mistakes, not user input:
    /// the caller has just measured these chains.
    pub(crate) fn transpose_cross(
        &mut self,
        outer: RowGap,
        at: [usize; 2],
        bounds: [&[f32]; 2],
        stack_fraction: f32,
    ) {
        let (outer_horizontal, kids, shares) = {
            let row = self[outer.row]
                .get_row()
                .expect("a transposition is rooted at a gap of a row");
            assert!(
                row.has_gap(outer.gap),
                "gap {} was named on a row with {} gaps",
                outer.gap.0,
                row.gap_count()
            );
            (
                row.is_horizontal(),
                row.children().to_vec(),
                row.shares().to_vec(),
            )
        };
        let inner_horizontal = !outer_horizontal;
        let k = outer.gap.0;
        // Exactly two, whatever the row holds — that is what a gap is. `at`, `bounds` and
        // `chains` are pairs for the same reason, and none of them is waiting for a row.
        let chains = [
            self.chain(kids[k], inner_horizontal),
            self.chain(kids[k + 1], inner_horizontal),
        ];

        for (c, chain) in chains.iter().enumerate() {
            assert_eq!(
                bounds[c].len(),
                chain.parts.len() + 1,
                "chain {c} has {} parts, so it has {} boundaries",
                chain.parts.len(),
                chain.parts.len() + 1
            );
            assert!(
                at[c] < chain.dividers.len(),
                "the crossing names divider {} of chain {c}, which has {}",
                at[c],
                chain.dividers.len()
            );
        }

        // The crossing line, along the chains' axis. Averaged: the two dividers are allowed to
        // differ by the caller's tolerance, that being the point of the tolerance.
        let line = 0.5 * (bounds[0][at[0] + 1] + bounds[1][at[1] + 1]);
        let span_start = bounds[0][0];
        let span_end = bounds[0][bounds[0].len() - 1];
        let cross_fraction = (line - span_start) / (span_end - span_start);

        // Rows the rebuild may reuse: the ones inside the two chains, each once. A row of three
        // contributes two dividers and is one node, so the list is deduplicated — which is why
        // this is no longer an exact "pool" with arithmetic to match. The new shape is flatter
        // than the ladder it replaces in some places and deeper in others; `Tree::regroup`
        // allocates what is missing and frees what is left over, and the promise that survives
        // is the one about the *parts*, not about the node count.
        let mut pool = Vec::new();
        for chain in &chains {
            for divider in &chain.dividers {
                if !pool.contains(&divider.row) {
                    pool.push(divider.row);
                }
            }
        }
        let mut pool = pool.into_iter();

        let near = rebuild_half(
            &chains,
            bounds,
            [0, 0],
            at,
            outer_horizontal,
            stack_fraction,
            &mut pool,
        );
        let far = rebuild_half(
            &chains,
            bounds,
            [at[0] + 1, at[1] + 1],
            [chains[0].parts.len() - 1, chains[1].parts.len() - 1],
            outer_horizontal,
            stack_fraction,
            &mut pool,
        );

        let shape = if kids.len() == 2 {
            // The replacement is the whole row, so the row becomes it — the same node, turned
            // by 90°, which is what this operation was before a row could hold three.
            Regroup::pair(
                Some(outer.row),
                inner_horizontal,
                cross_fraction,
                [near, far],
            )
        } else {
            let merged = Regroup::pair(pool.next(), inner_horizontal, cross_fraction, [near, far]);
            let mut children = Vec::with_capacity(kids.len() - 1);
            let mut merged_shares = Vec::with_capacity(kids.len() - 1);
            let mut merged = Some(merged);
            for (index, (&child, &share)) in kids.iter().zip(&shares).enumerate() {
                if index == k {
                    children.push(merged.take().expect("one gap, one replacement"));
                    // The two neighbours' room, added: the replacement occupies exactly what
                    // they occupied, so no other boundary of the row moves.
                    merged_shares.push(Share(share.0 + shares[k + 1].0));
                } else if index != k + 1 {
                    children.push(Regroup::Keep(child));
                    merged_shares.push(share);
                }
            }
            Regroup::Row {
                id: Some(outer.row),
                horizontal: outer_horizontal,
                shares: merged_shares,
                children,
            }
        };
        // Through `regroup` rather than by assigning `Node`s: subtrees change parent here, and a
        // child's back-pointer and the subtree's collapsing bookkeeping live outside the `Node`
        // being assigned.
        self.regroup(outer.row, &shape);
    }
}

/// One half of a transposed cross: what the two chains each had on one side of the crossing
/// line, stacked along the gap's old axis.
#[allow(clippy::too_many_arguments)]
fn rebuild_half(
    chains: &[Chain; 2],
    bounds: [&[f32]; 2],
    from: [usize; 2],
    to: [usize; 2],
    outer_horizontal: bool,
    stack_fraction: f32,
    pool: &mut impl Iterator<Item = NodeId>,
) -> Regroup {
    let inner_horizontal = !outer_horizontal;
    Regroup::pair(
        pool.next(),
        outer_horizontal,
        stack_fraction,
        [
            rebuild_run(
                &chains[0],
                bounds[0],
                from[0],
                to[0],
                inner_horizontal,
                pool,
            ),
            rebuild_run(
                &chains[1],
                bounds[1],
                from[1],
                to[1],
                inner_horizontal,
                pool,
            ),
        ],
    )
}

/// Parts `from..=to` of one chain, as **one row** — the shape they read as on screen.
///
/// A right-leaning ladder of pairs is what this built while a row held two, and the nesting was
/// free because any tree of splits at the same boundaries draws the same picture. It is not free
/// under the hand: the outer boundary of a ladder drags the inner one along with it, which is the
/// complaint this whole plan started from. One row keeps the picture and drops that.
///
/// What is not free either way is the weights, and those come straight off the measured
/// boundaries: part `i` asks for the length it already has.
fn rebuild_run(
    chain: &Chain,
    bounds: &[f32],
    from: usize,
    to: usize,
    horizontal: bool,
    pool: &mut impl Iterator<Item = NodeId>,
) -> Regroup {
    if from == to {
        return Regroup::Keep(chain.parts[from]);
    }
    Regroup::Row {
        id: pool.next(),
        horizontal,
        shares: (from..=to)
            .map(|index| Share(bounds[index + 1] - bounds[index]))
            .collect(),
        children: (from..=to)
            .map(|index| Regroup::Keep(chain.parts[index]))
            .collect(),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::core::tree::{GapIndex, Node, Split};

    /// The only gap of a row of two — every `outer` in this module is one, and a crossing is
    /// addressed by the gap it lies on.
    fn gap_of(row: NodeId) -> RowGap {
        RowGap {
            row,
            gap: GapIndex(0),
        }
    }

    /// Four leaves in a 2x2, grouped as two columns: `outer` cuts left from right, and each
    /// side is a stack. Returns the tree, `outer`, and the leaves clockwise from the top left:
    /// `[a, b, c, d]` for
    ///
    /// ```text
    ///   a │ c
    ///  ───┼───
    ///   b │ d
    /// ```
    fn two_columns() -> (Tree<u32>, NodeId, [NodeId; 4]) {
        let mut tree = Tree::new(vec![0u32]);
        let first = tree.root().unwrap();
        // `split` hands back the node that was there and the one that joined it; the split
        // *between* them is a new node, and here it is the tree's new root.
        let [left, right] = tree.split(first, Split::Right, 0.5, Node::leaf(2u32));
        let outer = tree.root().unwrap();
        let [a, b] = tree.split(left, Split::Below, 0.5, Node::leaf(1u32));
        let [c, d] = tree.split(right, Split::Below, 0.5, Node::leaf(3u32));
        (tree, outer, [a, b, c, d])
    }

    /// The boundaries of a chain of `n` equal parts spanning `0..=1`.
    fn even(n: usize) -> Vec<f32> {
        (0..=n).map(|k| k as f32 / n as f32).collect()
    }

    /// A chain is what the eye reads as one row or column, whatever the nesting: the walk stops
    /// at the first node of the other orientation and does not look inside it.
    #[test]
    fn a_chain_stops_at_the_first_node_of_the_other_orientation() {
        let (tree, outer, [a, b, c, d]) = two_columns();

        let columns = tree.chain(outer, true);
        assert_eq!(columns.parts.len(), 2, "two columns side by side");
        assert_eq!(
            columns.dividers,
            vec![RowGap {
                row: outer,
                gap: crate::core::tree::GapIndex(0)
            }],
            "with `outer`'s one gap between them"
        );

        // Each column is a chain of its own, along the other axis.
        let left = tree.chain(columns.parts[0], false);
        assert_eq!(left.parts, vec![a, b]);
        let right = tree.chain(columns.parts[1], false);
        assert_eq!(right.parts, vec![c, d]);
    }

    /// The 2x2 case: two columns become two rows, every leaf keeps its quadrant, and not one
    /// node is created or dropped — the rebuild is made out of the splits it took apart.
    #[test]
    fn transposing_a_cross_regroups_the_same_four_leaves() {
        let (mut tree, outer, [a, b, c, d]) = two_columns();
        let before = tree.len();

        tree.transpose_cross(gap_of(outer), [0, 0], [&even(2), &even(2)], 0.5);

        assert_eq!(tree.validate(), Ok(()), "the tree is still well formed");
        assert_eq!(tree.len(), before, "no node was created or dropped");

        // `outer` now cuts top from bottom, and each half holds what the two columns had on
        // its side of the line: the top row is `a` beside `c`, the bottom `b` beside `d`.
        assert!(tree[outer].is_vertical(), "the grouping turned by 90°");
        let rows = tree.chain(outer, false);
        assert_eq!(rows.parts.len(), 2);
        assert_eq!(tree.chain(rows.parts[0], true).parts, vec![a, c]);
        assert_eq!(tree.chain(rows.parts[1], true).parts, vec![b, d]);
    }

    /// Pressing the same crossing again brings the original grouping back — the property the
    /// whole edit rests on, and the one a user checks first.
    #[test]
    fn transposing_twice_returns_to_the_grouping_it_started_from() {
        let (mut tree, outer, [a, b, c, d]) = two_columns();

        tree.transpose_cross(gap_of(outer), [0, 0], [&even(2), &even(2)], 0.5);
        tree.transpose_cross(gap_of(outer), [0, 0], [&even(2), &even(2)], 0.5);

        assert_eq!(tree.validate(), Ok(()));
        assert!(tree[outer].is_horizontal(), "back to two columns");
        let columns = tree.chain(outer, true);
        assert_eq!(tree.chain(columns.parts[0], false).parts, vec![a, b]);
        assert_eq!(tree.chain(columns.parts[1], false).parts, vec![c, d]);
    }

    /// Chains longer than two: the crossing carries the whole of `outer`, and each half keeps
    /// the parts on its own side of the line — in order, however many there are.
    #[test]
    fn a_crossing_cuts_chains_of_any_length_at_its_own_line() {
        // Left column of three, right column of two, crossing at the left column's *second*
        // divider and the right column's only one.
        let mut tree = Tree::new(vec![0u32]);
        let first = tree.root().unwrap();
        let [left, right] = tree.split(first, Split::Right, 0.5, Node::leaf(10u32));
        let outer = tree.root().unwrap();
        let [a, rest] = tree.split(left, Split::Below, 1.0 / 3.0, Node::leaf(1u32));
        let [b, c] = tree.split(rest, Split::Below, 0.5, Node::leaf(2u32));
        let [d, e] = tree.split(right, Split::Below, 2.0 / 3.0, Node::leaf(11u32));

        tree.transpose_cross(gap_of(outer), [1, 0], [&even(3), &even(2)], 0.5);

        assert_eq!(tree.validate(), Ok(()));
        let rows = tree.chain(outer, false);
        assert_eq!(rows.parts.len(), 2, "the line cuts `outer` in two");

        // Above the line the left column kept two parts, so they are a chain of their own and
        // the walk stops at it; the right column kept one, which is `d` itself.
        let top = tree.chain(rows.parts[0], true);
        assert_eq!(top.parts.len(), 2, "what each column had above the line");
        assert_eq!(tree.chain(top.parts[0], false).parts, vec![a, b]);
        assert_eq!(top.parts[1], d);

        // Below it both columns kept exactly one part, so the halves are those two leaves.
        assert_eq!(
            tree.chain(rows.parts[1], true).parts,
            vec![c, e],
            "below it: what each column had left"
        );
    }

    /// The fractions are read off the boundaries handed in, not invented: a line three quarters
    /// of the way down is where the transposed grouping cuts `outer`.
    #[test]
    fn the_new_grouping_cuts_where_the_measured_line_was() {
        let (mut tree, outer, _) = two_columns();

        // Both columns divided at 0.75 of their height, in a chain spanning 0..=1.
        let bounds = vec![0.0, 0.75, 1.0];
        tree.transpose_cross(gap_of(outer), [0, 0], [&bounds, &bounds], 0.5);

        let fraction = tree[outer]
            .get_row()
            .expect("`outer` stays a split")
            .fraction();
        assert!(
            (fraction - 0.75).abs() < 1e-6,
            "the line was at 0.75 of the span, and the cut is there: {fraction}"
        );
    }
}
