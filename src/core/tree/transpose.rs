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
use crate::core::tree::{NodeId, Tree};

/// A chain of same-oriented splits, flattened in screen order.
///
/// `dividers[k]` is the split whose boundary falls between `parts[k]` and `parts[k + 1]`, so
/// there is always exactly one fewer divider than there are parts. The dividers double as the
/// pool of ids a transposition rebuilds the chain out of: a chain taken apart and re-nested
/// needs exactly as many splits as it had, and reusing its own keeps every id, tab and focus
/// flag below it untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chain {
    /// The subtrees hanging off the chain, in screen order (left to right, or top to bottom).
    pub(crate) parts: Vec<NodeId>,

    /// The `parts.len() - 1` splits between them, in the same order.
    pub(crate) dividers: Vec<NodeId>,
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
        // A pair on purpose, not a loop over the children: `dividers` names a divider by the
        // *node* of the split that draws it, which works only while a split draws exactly one.
        // A row of three written as a loop here would push the same id twice and call it two
        // dividers. Stage 5 of the n-ary plan gives a divider its own address (a gap in a row),
        // and that is what turns this into a loop.
        let [first, second] = self[node]
            .get_row()
            .expect("a node in a chain is a split")
            .children_pair();
        self.collect_chain(first, horizontal, chain);
        chain.dividers.push(node);
        self.collect_chain(second, horizontal, chain);
    }

    /// Regroup around the crossing at `at`, keeping every leaf exactly where it is on screen.
    ///
    /// `outer` is the split between the two chains, and `at` names the divider of each that the
    /// crossing is made of: `at[0]` counts along the first chain, `at[1]` along the second. The
    /// line through that crossing runs the full extent of both chains, so it can carry the whole
    /// of `outer`: afterwards `outer` is cut *by that line* into two halves, and each half stacks
    /// what the two chains had on its side of it.
    ///
    /// The 2x2 case — one divider per chain, both chains two parts — is this with every
    /// sub-chain of length one, and comes out with `outer`'s two old children still playing the
    /// two halves. The general case has `n + m` rectangles and no "swap" reading, but the promise
    /// is the same: nothing moves except the two dividers that were out of line, which become one
    /// line — their average — so each moves by half of whatever gap the caller's alignment
    /// tolerance let through.
    ///
    /// `bounds[c]` are the `parts + 1` boundaries of chain `c` along its own axis, ascending:
    /// its two outer edges with each divider's position in between. `stack_fraction` is the one
    /// number from the *other* axis — where `outer`'s own boundary sits between its two edges —
    /// because that is the share each rebuilt half keeps for the first chain.
    ///
    /// # Panics
    ///
    /// If `outer` is not a split, if `at` does not name a divider of both chains, or if `bounds`
    /// does not describe them. All four are caller mistakes, not user input: the caller has just
    /// measured these chains.
    pub(crate) fn transpose_cross(
        &mut self,
        outer: NodeId,
        at: [usize; 2],
        bounds: [&[f32]; 2],
        stack_fraction: f32,
    ) {
        let outer_horizontal = self[outer].is_horizontal();
        let inner_horizontal = !outer_horizontal;
        // A pair, and honestly so at every level of this function: a crossing is made of
        // exactly two chains, which is why `at`, `bounds` and `chains` are pairs as well. A
        // transposition of a row of three is a different gesture, not a wider version of this
        // one, so nothing here is waiting for a row.
        let [first, second] = self[outer]
            .get_row()
            .expect("a transposition is rooted at the split between the two chains")
            .children_pair();
        let chains = [
            self.chain(first, inner_horizontal),
            self.chain(second, inner_horizontal),
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

        // The pool of split ids the new shape is built out of: exactly the two chains being
        // taken apart. `outer` is not in it — it stays where it is, because its own parent
        // points at it — and the arithmetic leaves none over: `(n - 1) + (m - 1)` ids in, two
        // halves plus `(k - 1) + (n - k - 1) + (l - 1) + (m - l - 1)` chain splits out.
        let mut pool = chains[0]
            .dividers
            .iter()
            .chain(&chains[1].dividers)
            .copied();

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
        assert!(
            pool.next().is_none(),
            "a transposition needs exactly as many splits as the chains it took apart"
        );

        let shape = Regroup::Split {
            id: outer,
            horizontal: inner_horizontal,
            fraction: cross_fraction,
            children: [Box::new(near), Box::new(far)],
        };
        // Through `regroup` rather than by assigning `Node`s: subtrees change parent here, and a
        // child's back-pointer and the subtree's collapsing bookkeeping live outside the `Node`
        // being assigned.
        self.regroup(outer, &shape);
    }
}

/// One half of a transposed cross: what the two chains each had on one side of the crossing
/// line, stacked along `outer`'s old axis.
///
/// The half itself reuses a split from `pool`, and so does every sub-chain inside it, in that
/// order — which is what makes the 2x2 case come out with `outer`'s two old children still
/// playing the two halves.
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
                &chains[0],
                bounds[0],
                from[0],
                to[0],
                inner_horizontal,
                pool,
            )),
            Box::new(rebuild_chain(
                &chains[1],
                bounds[1],
                from[1],
                to[1],
                inner_horizontal,
                pool,
            )),
        ],
    }
}

/// Parts `from..=to` of one chain, re-nested right-leaning: `part(from)` beside the rest.
///
/// Right-leaning is the cheapest nesting there is — each split's two sides hold at least one
/// whole part — and which nesting is chosen is free, because any tree of splits at the same
/// boundaries draws the same picture. What is not free is the fractions, and those come straight
/// off the measured boundaries.
fn rebuild_chain(
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
    let id = pool
        .next()
        .expect("a chain is rebuilt out of exactly the splits it was taken apart from");
    Regroup::Split {
        id,
        horizontal,
        fraction: (bounds[from + 1] - bounds[from]) / (bounds[to + 1] - bounds[from]),
        children: [
            Box::new(Regroup::Keep(chain.parts[from])),
            Box::new(rebuild_chain(chain, bounds, from + 1, to, horizontal, pool)),
        ],
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::core::tree::{Node, Split};

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
        assert_eq!(columns.dividers, vec![outer], "with `outer` between them");

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

        tree.transpose_cross(outer, [0, 0], [&even(2), &even(2)], 0.5);

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

        tree.transpose_cross(outer, [0, 0], [&even(2), &even(2)], 0.5);
        tree.transpose_cross(outer, [0, 0], [&even(2), &even(2)], 0.5);

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

        tree.transpose_cross(outer, [1, 0], [&even(3), &even(2)], 0.5);

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
        tree.transpose_cross(outer, [0, 0], [&bounds, &bounds], 0.5);

        let fraction = tree[outer]
            .get_row()
            .expect("`outer` stays a split")
            .fraction;
        assert!(
            (fraction - 0.75).abs() < 1e-6,
            "the line was at 0.75 of the span, and the cut is there: {fraction}"
        );
    }
}
