//! Rewriting the *shape* of a subtree while leaving everything hanging off it untouched.
//!
//! The one operation in this crate that regroups nodes rather than adding or removing them:
//! the same leaves end up under different parents, in a different nesting, and every id, tab,
//! focus flag and collapsed count below the rewrite survives it. Only the rows above them are
//! rebuilt.
//!
//! # What is guaranteed, and what is not
//!
//! The **kept subtrees** are the promise: exactly the ones that were under the rewritten root
//! before are under it after, each once, each with its id, its tabs and its focus intact. That is
//! the property a user can see, and it is checked here rather than trusted.
//!
//! The **rows above them** are material, not promises. A regrouping reuses the row nodes it finds
//! wherever it can, allocates when the new shape needs more of them, and frees the ones it no
//! longer needs. It did not always: while every row held exactly two children, the same picture
//! always took the same number of rows, so "reuse exactly what was there" was both achievable and
//! a useful check. A row of `n` broke that arithmetic in both directions at once — one row of
//! three replaces two nested pairs, and cutting that row into a two and a one needs a node the
//! pair-shaped tree already had. Insisting on the old count now would mean refusing to build the
//! flatter shape, which is the whole point of the feature.

use std::collections::HashSet;

use crate::core::tree::arena::NodeEntry;
use crate::core::tree::{Node, NodeId, RowNode, Share, Tree};

/// The shape to write over a subtree.
///
/// A tree of [`Regroup::Row`]s down to the subtrees that are being kept: each `Row` is a row to
/// write, either over a row node that exists *now* (`id: Some`) or into a freshly allocated one
/// (`id: None`); each [`Regroup::Keep`] names a node that is not touched at all and only changes
/// where it hangs.
///
/// Naming the reused ids explicitly, rather than letting [`Tree::regroup`] hand them all out, is
/// what lets the caller decide which node keeps which role — a divider the hand is holding stays
/// the same node across the regrouping only if the caller says so.
#[derive(Debug)]
pub(crate) enum Regroup {
    /// A subtree left exactly as it is: same id, same contents, same shape.
    Keep(NodeId),

    /// A row, written over the node `id` names or into a new one.
    Row {
        /// A node that is a row in the tree right now, and stays one — everything else about it
        /// is replaced. `None` allocates: the new shape needs a row the old one did not have.
        ///
        /// The root of a regrouping is always `Some`, because its own parent already points at
        /// it (see [`Tree::regroup`]).
        id: Option<NodeId>,
        /// `true` for children side by side, `false` for children stacked — the same flag
        /// [`RowNode::is_horizontal`] answers.
        horizontal: bool,
        /// One weight per child, in `children` order.
        shares: Vec<Share>,
        children: Vec<Regroup>,
    },
}

impl Regroup {
    /// A row of two, from the boundary between them — the spelling a crossing speaks in.
    pub(crate) fn pair(
        id: Option<NodeId>,
        horizontal: bool,
        fraction: f32,
        children: [Regroup; 2],
    ) -> Self {
        Regroup::Row {
            id,
            horizontal,
            shares: vec![Share(fraction), Share(1.0 - fraction)],
            children: children.into_iter().collect(),
        }
    }

    /// Every node the shape keeps, and every row id it names.
    fn census(&self, rows: &mut Vec<NodeId>, keeps: &mut Vec<NodeId>) {
        match self {
            Regroup::Keep(id) => keeps.push(*id),
            Regroup::Row { id, children, .. } => {
                rows.extend(id);
                for child in children {
                    child.census(rows, keeps);
                }
            }
        }
    }

    /// Writes the shape into the tree, children before parents, and answers where it landed.
    ///
    /// Children first because a row is built out of its children's ids, and a row that has to be
    /// allocated has no id to hand down beforehand.
    fn write<Tab>(&self, tree: &mut Tree<Tab>) -> NodeId {
        let Regroup::Row {
            id,
            horizontal,
            shares,
            children,
        } = self
        else {
            let Regroup::Keep(id) = self else {
                unreachable!("a shape is a row or a keep")
            };
            return *id;
        };

        let child_ids: Vec<NodeId> = children.iter().map(|child| child.write(tree)).collect();
        let node = Node::Row(RowNode::new(*horizontal, child_ids.clone(), shares.clone()));
        let row = match id {
            // Assigning the `Node` and not the whole entry: the reused node's *parent* link is
            // still the one that points at it from above, and the root's especially so.
            Some(existing) => {
                tree[*existing] = node;
                *existing
            }
            None => tree.nodes.insert(NodeEntry { parent: None, node }),
        };

        // The back-pointers, which assigning a `Node` cannot carry: a `Node` holds its
        // children's ids, but a child holds its parent's, and that half lives in the arena.
        for child in child_ids {
            tree.nodes
                .get_mut(child)
                .expect("a regrouped child is a live node")
                .parent = Some(row);
        }
        row
    }
}

impl<Tab> Tree<Tab> {
    /// Rewrites the subtree at `root` into `shape`.
    ///
    /// The contract, and the reason this is one operation rather than a handful of
    /// `self[id] = node` assignments: `shape` must **keep exactly the subtrees that are under
    /// `root` right now** — each once, none invented and none dropped — and it may reuse as a
    /// row only a node that is one today. It may nest the kept subtrees differently, give them
    /// different orientations and weights, and need more or fewer rows than it found; rows it
    /// does not name are freed.
    ///
    /// What the assignments cannot do on their own is the other half of a row — each child's
    /// back-pointer to its parent, and the collapsing bookkeeping of everything above — and a
    /// tree that draws correctly with a stale back-pointer stays quiet until something walks
    /// *up* from a moved node and panics one gesture removed from the cause. See
    /// `Tree::validate`'s `ParentLinkBroken`.
    ///
    /// # Panics
    ///
    /// If `shape` does not sit at `root`, mentions an id twice, reuses as a row something that
    /// is not one under `root`, or does not keep exactly what is under `root` today.
    pub(crate) fn regroup(&mut self, root: NodeId, shape: &Regroup) {
        assert!(
            matches!(shape, Regroup::Row { id: Some(id), .. } if *id == root),
            "a regrouping is rooted at the node it replaces: it cannot move `root` itself, \
             whose parent still points at it, nor allocate a new node in its place"
        );

        let mut new_rows = Vec::new();
        let mut new_keeps = Vec::new();
        shape.census(&mut new_rows, &mut new_keeps);

        let mentioned: HashSet<NodeId> = new_rows.iter().chain(&new_keeps).copied().collect();
        assert_eq!(
            mentioned.len(),
            new_rows.len() + new_keeps.len(),
            "a regrouping mentions each node once: an id used twice would give one node two \
             parents"
        );

        // What is under `root` today, cut where the shape cuts it: descend until a node the
        // shape *keeps*, and stop there without looking inside it. Everything strictly above
        // the frontier is a row this regrouping owns — to reuse or to free.
        //
        // The frontier is the keeps and not the reused ids, which is the difference a row of `n`
        // makes: a shape may now drop a row it found, and the walk still has to go through it to
        // reach the subtrees underneath.
        let kept: HashSet<NodeId> = new_keeps.iter().copied().collect();
        let mut old_rows = Vec::new();
        let mut old_keeps = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if kept.contains(&id) {
                old_keeps.push(id);
                continue;
            }
            let children = self
                .children(id)
                .expect("a node under a regrouped root is either kept whole or is a row");
            old_rows.push(id);
            stack.extend(children.iter().copied());
        }

        let sorted = |mut ids: Vec<NodeId>| {
            ids.sort_unstable();
            ids
        };
        assert_eq!(
            sorted(new_keeps),
            sorted(old_keeps.clone()),
            "a regrouping regroups the subtrees it was given; it may not invent, drop or \
             duplicate one"
        );
        let available: HashSet<NodeId> = old_rows.iter().copied().collect();
        assert!(
            new_rows.iter().all(|id| available.contains(id)),
            "a regrouping reuses as a row only a node that is one under `root`: {new_rows:?} \
             against {old_rows:?}"
        );

        shape.write(self);

        // The rows the new shape had no use for. Freed rather than left in the arena: nothing
        // points at them any more, and an unreachable node is an `OrphanNode` violation.
        let reused: HashSet<NodeId> = new_rows.into_iter().collect();
        for id in old_rows {
            if !reused.contains(&id) {
                self.nodes.remove(id);
            }
        }

        // Every rebuilt row starts with an empty count, and the subtrees that moved carry
        // theirs with them, so the counts above are settled in one sweep rather than patched
        // per node.
        self.recompute_collapsed();
    }
}
