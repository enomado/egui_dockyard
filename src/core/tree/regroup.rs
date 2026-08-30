//! Rewriting the *shape* of a subtree while leaving everything hanging off it untouched.
//!
//! The one operation in this crate that regroups nodes rather than adding or removing them:
//! the same leaves end up under different parents, in a different nesting, and every id, tab,
//! focus flag and collapsed count below the rewrite survives it. Only the splits above them
//! are rebuilt — out of the very split nodes that were there before, so the rewrite neither
//! allocates nor frees a single arena slot.

use std::collections::HashSet;

use crate::core::tree::{Node, NodeId, SplitNode, Tree};

/// The shape to write over a subtree.
///
/// A tree of [`Regroup::Split`]s down to the subtrees that are being kept: each `Split` names a
/// split node that exists *now* and will be rebuilt with a new orientation, fraction and pair of
/// children; each [`Regroup::Keep`] names a node that is not touched at all and only changes
/// where it hangs.
///
/// Naming the reused ids explicitly, rather than letting [`Tree::regroup`] hand them out, is
/// what lets the caller decide which node keeps which role — and it is what makes the "may not
/// invent, drop or duplicate" check below possible at all.
#[derive(Debug)]
pub(crate) enum Regroup {
    /// A subtree left exactly as it is: same id, same contents, same shape.
    Keep(NodeId),

    /// A split node, rebuilt in place of the one `id` names now.
    Split {
        /// A node that is a split in the tree right now. It stays a split; everything else
        /// about it is replaced.
        id: NodeId,
        /// `true` for [`Node::Horizontal`] (children side by side), `false` for
        /// [`Node::Vertical`] (children stacked).
        horizontal: bool,
        /// The share of the parent rectangle taken by the first child.
        fraction: f32,
        children: [Box<Regroup>; 2],
    },
}

impl Regroup {
    /// The node this shape sits at, whichever kind it is.
    pub(crate) fn id(&self) -> NodeId {
        match self {
            Regroup::Keep(id) => *id,
            Regroup::Split { id, .. } => *id,
        }
    }

    /// Every id the shape mentions, split ids and kept ids kept apart.
    fn census(&self, splits: &mut Vec<NodeId>, keeps: &mut Vec<NodeId>) {
        match self {
            Regroup::Keep(id) => keeps.push(*id),
            Regroup::Split { id, children, .. } => {
                splits.push(*id);
                for child in children {
                    child.census(splits, keeps);
                }
            }
        }
    }

    /// Writes the shape into the tree, parents before children.
    ///
    /// Only `Split` nodes are written; a `Keep` is reached, its parent link is set by whoever
    /// owns it, and nothing else about it is read or touched.
    fn write<Tab>(&self, tree: &mut Tree<Tab>) {
        let Regroup::Split {
            id,
            horizontal,
            fraction,
            children,
        } = self
        else {
            return;
        };

        let child_ids = [children[0].id(), children[1].id()];
        let split = SplitNode::new(child_ids, *fraction);
        tree[*id] = if *horizontal {
            Node::Horizontal(split)
        } else {
            Node::Vertical(split)
        };

        // The back-pointers, which assigning a `Node` cannot carry: a `Node` holds its
        // children's ids, but a child holds its parent's, and that half lives in the arena.
        for child in child_ids {
            tree.nodes
                .get_mut(child)
                .expect("a regrouped child is a live node")
                .parent = Some(*id);
        }

        for child in children {
            child.write(tree);
        }
    }
}

impl<Tab> Tree<Tab> {
    /// Rewrites the subtree at `root` into `shape`.
    ///
    /// The contract, and the reason this is one operation rather than a handful of
    /// `self[id] = node` assignments: `shape` must be built out of *exactly* the material that
    /// is under `root` right now — the same split nodes, reused as splits, and the same kept
    /// subtrees, reused as leaves of the shape. It may nest them differently and give them
    /// different orientations and fractions; it may not invent, drop or duplicate one. What
    /// the assignments cannot do on their own is the other half of a split — each child's
    /// back-pointer to its parent, and the collapsing bookkeeping of everything above — and a
    /// tree that draws correctly with a stale back-pointer stays quiet until something walks
    /// *up* from a moved node and panics one gesture removed from the cause. See
    /// `Tree::validate`'s `ParentLinkBroken`.
    ///
    /// # Panics
    ///
    /// If `shape` does not sit at `root`, mentions an id twice, reuses as a split something
    /// that is not one, or does not account for exactly what is under `root` today.
    pub(crate) fn regroup(&mut self, root: NodeId, shape: &Regroup) {
        assert!(
            matches!(shape, Regroup::Split { id, .. } if *id == root),
            "a regrouping is rooted at the node it replaces: it cannot move `root` itself, \
             whose parent still points at it"
        );

        let mut new_splits = Vec::new();
        let mut new_keeps = Vec::new();
        shape.census(&mut new_splits, &mut new_keeps);

        let mentioned: HashSet<NodeId> = new_splits.iter().chain(&new_keeps).copied().collect();
        assert_eq!(
            mentioned.len(),
            new_splits.len() + new_keeps.len(),
            "a regrouping mentions each node once: an id used twice would give one node two \
             parents"
        );

        // What is under `root` today, cut the same way the shape cuts it: descend through
        // exactly those nodes the shape reuses as splits, and stop wherever it does not. The
        // stops are what the shape has to be keeping.
        let reused: HashSet<NodeId> = new_splits.iter().copied().collect();
        let mut old_splits = Vec::new();
        let mut old_keeps = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if reused.contains(&id) {
                let children = self
                    .children(id)
                    .expect("a regrouping reuses as a split only a node that is one");
                old_splits.push(id);
                stack.extend(children.iter().copied());
            } else {
                old_keeps.push(id);
            }
        }

        let sorted = |mut ids: Vec<NodeId>| {
            ids.sort_unstable();
            ids
        };
        assert_eq!(
            sorted(new_splits.clone()),
            sorted(old_splits),
            "a regrouping rebuilds the splits it was given; it may not invent, drop or \
             duplicate one"
        );
        assert_eq!(
            sorted(new_keeps),
            sorted(old_keeps),
            "a regrouping regroups the subtrees it was given; it may not invent, drop or \
             duplicate one"
        );

        shape.write(self);

        // Every rebuilt split starts with an empty count, and the subtrees that moved carry
        // theirs with them, so the counts above are settled in one sweep rather than patched
        // per node.
        self.recompute_collapsed();
    }
}
