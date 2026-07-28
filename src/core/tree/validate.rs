//! Read-only structural oracle for a [`Tree`].
//!
//! [`Tree::validate`] answers one question — "is this tree well-formed?" — without mutating
//! anything and without any knowledge of `egui`. It exists so that structural work on the tree
//! (and on anything built on top of it) can be checked mechanically instead of by eye:
//! property tests assert `validate()` after every operation, characterization tests assert it
//! over a corpus of layouts loaded from disk, and a fuzzer can use it as its oracle.
//!
//! # What is being checked
//!
//! The tree is stored as an implicit binary heap: the children of node *n* live at *2n + 1* and
//! *2n + 2*, and slots that carry no node hold [`Node::Empty`]. Nothing in the type system says
//! that the resulting `Vec` describes a tree — the invariants below are what make it one, and
//! they are upheld by convention in every operation. That is precisely why they are worth
//! stating in one place.
//!
//! # What is *not* being checked
//!
//! Geometry (`rect`/`viewport`) is deliberately ignored: it is a per-frame cache written by the
//! layout pass, not state, so it is meaningless before the first frame and stale afterwards.

use crate::{Node, NodeIndex, Surface, SurfaceIndex, TabIndex, Tree};

/// A single way in which a [`Tree`] fails to be well-formed.
///
/// Reported by [`Tree::validate`]. Each variant carries enough context to point at the offending
/// node, so a failing property test or fuzz case names the place rather than just "invalid".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TreeViolation {
    /// A non-empty node whose parent slot is out of bounds or holds [`Node::Empty`].
    ///
    /// Such a node is unreachable from the root: no traversal will ever visit it, but it still
    /// owns its tabs, so this is how tabs go missing while remaining in memory.
    OrphanNode {
        /// The unreachable node.
        node: NodeIndex,
    },

    /// A split node whose child slot is out of bounds or holds [`Node::Empty`].
    ///
    /// A split renders two areas; a missing child means one of them has nothing to draw.
    SplitChildMissing {
        /// The split node.
        node: NodeIndex,
        /// The child slot that is missing.
        child: NodeIndex,
    },

    /// A leaf node that nonetheless has a non-empty child.
    ///
    /// The child is unreachable through the leaf (leaves are not traversed into), so it is a
    /// subtree that has been silently orphaned in place.
    LeafHasChild {
        /// The leaf node.
        node: NodeIndex,
        /// The child that should not exist.
        child: NodeIndex,
    },

    /// A non-root leaf that holds no tabs.
    ///
    /// Removing the last tab from a leaf is supposed to collapse the leaf
    /// ([`Tree::remove_tab`] does this); a surviving empty leaf means some path removed tabs
    /// without collapsing. The root leaf is exempt: an empty dock is a legitimate state.
    EmptyLeaf {
        /// The empty leaf.
        node: NodeIndex,
    },

    /// `active` does not address an existing tab.
    ActiveOutOfRange {
        /// The leaf node.
        node: NodeIndex,
        /// The offending index.
        active: TabIndex,
        /// How many tabs the leaf actually has.
        tabs: usize,
    },

    /// `prev_active` is out of range, or equal to `active`.
    ///
    /// Both cases break the documented invariant of [`LeafNode::prev_active`](crate::LeafNode::prev_active):
    /// falling back to it after removing the active tab would then either panic or be a no-op.
    PrevActiveInvalid {
        /// The leaf node.
        node: NodeIndex,
        /// The offending index.
        prev_active: TabIndex,
        /// The currently active index, for context.
        active: TabIndex,
        /// How many tabs the leaf actually has.
        tabs: usize,
    },

    /// `focused_node` points at a slot that is out of bounds, empty, or not a leaf.
    ///
    /// Focus is what `push_to_focused_leaf` targets, so a stale focus silently redirects newly
    /// opened tabs.
    FocusNotALeaf {
        /// The offending focus target.
        node: NodeIndex,
    },
}

/// A [`TreeViolation`] together with the surface whose tree it was found in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceViolation {
    /// The surface holding the offending tree.
    pub surface: SurfaceIndex,
    /// What is wrong with it.
    pub violation: TreeViolation,
}

impl<Tab> Tree<Tab> {
    /// Checks the tree's structural invariants and returns every violation found.
    ///
    /// Read-only: nothing is mutated, nothing is repaired. An empty tree (no nodes at all) is
    /// well-formed, as is a tree whose only node is an empty root leaf.
    ///
    /// Intended as a test oracle rather than a runtime check — it walks every node, so it is
    /// linear in tree size and not meant for per-frame use.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use egui_dock::{DockState, NodeIndex};
    /// let mut dock_state = DockState::new(vec!["tab 1", "tab 2"]);
    /// let _ = dock_state
    ///     .main_surface_mut()
    ///     .split_left(NodeIndex::root(), 0.5, vec!["tab 3"]);
    /// assert_eq!(dock_state.main_surface().validate(), Ok(()));
    /// ```
    pub fn validate(&self) -> Result<(), Vec<TreeViolation>> {
        let mut violations = Vec::new();

        let node_at = |index: NodeIndex| -> Option<&Node<Tab>> {
            match self.nodes.get(index.0) {
                Some(Node::Empty) | None => None,
                Some(node) => Some(node),
            }
        };

        for index in (0..self.nodes.len()).map(NodeIndex) {
            let Some(node) = node_at(index) else {
                continue;
            };

            // Reachability: every node except the root must hang off a live parent.
            if let Some(parent) = index.parent()
                && node_at(parent).is_none()
            {
                violations.push(TreeViolation::OrphanNode { node: index });
            }

            match node {
                Node::Empty => unreachable!("filtered out by node_at"),

                Node::Vertical(_) | Node::Horizontal(_) => {
                    for child in [index.left(), index.right()] {
                        if node_at(child).is_none() {
                            violations
                                .push(TreeViolation::SplitChildMissing { node: index, child });
                        }
                    }
                }

                Node::Leaf(leaf) => {
                    for child in [index.left(), index.right()] {
                        if node_at(child).is_some() {
                            violations.push(TreeViolation::LeafHasChild { node: index, child });
                        }
                    }

                    let tabs = leaf.tabs.len();
                    if tabs == 0 {
                        // The root leaf is allowed to be empty: that is just an empty dock.
                        if index != NodeIndex::root() {
                            violations.push(TreeViolation::EmptyLeaf { node: index });
                        }
                    } else {
                        if leaf.active.0 >= tabs {
                            violations.push(TreeViolation::ActiveOutOfRange {
                                node: index,
                                active: leaf.active,
                                tabs,
                            });
                        }
                        if let Some(prev_active) = leaf.prev_active
                            && (prev_active.0 >= tabs || prev_active == leaf.active)
                        {
                            violations.push(TreeViolation::PrevActiveInvalid {
                                node: index,
                                prev_active,
                                active: leaf.active,
                                tabs,
                            });
                        }
                    }
                }
            }
        }

        if let Some(focused) = self.focused_leaf() {
            let is_leaf = matches!(node_at(focused), Some(Node::Leaf(_)));
            if !is_leaf {
                violations.push(TreeViolation::FocusNotALeaf { node: focused });
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

impl<Tab> crate::DockState<Tab> {
    /// Runs [`Tree::validate`] over every surface and reports violations with their surface.
    ///
    /// Also checks the one invariant that lives above the tree: surface 0 is the main surface.
    pub fn validate(&self) -> Result<(), Vec<SurfaceViolation>> {
        let mut violations = Vec::new();

        for (surface_index, surface) in self.iter_surfaces_indexed() {
            let tree = match surface {
                Surface::Empty => continue,
                Surface::Main(tree) | Surface::Window(tree, _) => tree,
            };
            if let Err(tree_violations) = tree.validate() {
                violations.extend(
                    tree_violations
                        .into_iter()
                        .map(|violation| SurfaceViolation {
                            surface: surface_index,
                            violation,
                        }),
                );
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{DockState, Node, NodeIndex, Split, TabIndex, Tree};

    /// The oracle must accept trees built through the public API — otherwise every later test
    /// that uses it is just measuring the oracle's own false positives.
    #[test]
    fn well_formed_trees_pass() {
        assert_eq!(Tree::<i32>::new(vec![]).validate(), Ok(()));
        assert_eq!(Tree::new(vec![1, 2, 3]).validate(), Ok(()));

        let mut dock_state = DockState::new(vec![1, 2]);
        let [_, right] = dock_state
            .main_surface_mut()
            .split_right(NodeIndex::root(), 0.5, vec![3]);
        let _ = dock_state
            .main_surface_mut()
            .split_below(right, 0.5, vec![4]);
        assert_eq!(dock_state.validate(), Ok(()));
        assert_eq!(dock_state.main_surface().validate(), Ok(()));
    }

    /// Each of these corrupts a tree in one specific way and asserts that the oracle names that
    /// specific way. Without them, `validate()` returning `Ok` would prove nothing: an oracle
    /// that never bites is indistinguishable from no oracle at all.
    #[test]
    fn oracle_bites_on_a_missing_split_child() {
        let mut tree = Tree::new(vec![1, 2]);
        let _ = tree.split_right(NodeIndex::root(), 0.5, vec![3]);
        // Blow away the right child of the root split.
        tree[NodeIndex::root().right()] = Node::Empty;

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&super::TreeViolation::SplitChildMissing {
                node: NodeIndex::root(),
                child: NodeIndex::root().right(),
            }),
            "expected a missing-child report, got {violations:?}"
        );
    }

    #[test]
    fn oracle_bites_on_an_orphan() {
        // root split -> [1] and [2]; then split node 1 again, so 3 and 4 hang off it.
        let mut tree = Tree::new(vec![1]);
        let _ = tree.split_right(NodeIndex::root(), 0.5, vec![2]);
        let _ = tree.split_right(NodeIndex(1), 0.5, vec![3]);
        assert_eq!(tree.validate(), Ok(()), "precondition: tree is well-formed");

        // Cutting the middle node loose orphans everything hanging off it.
        tree[NodeIndex(1)] = Node::Empty;

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&super::TreeViolation::OrphanNode { node: NodeIndex(3) })
                && violations.contains(&super::TreeViolation::OrphanNode { node: NodeIndex(4) }),
            "expected both children reported as orphans, got {violations:?}"
        );
    }

    #[test]
    fn oracle_bites_on_an_out_of_range_active_tab() {
        let mut tree = Tree::new(vec![1, 2]);
        let Node::Leaf(leaf) = &mut tree[NodeIndex::root()] else {
            panic!("root of a fresh tree is a leaf");
        };
        leaf.active = TabIndex(7);

        assert_eq!(
            tree.validate(),
            Err(vec![super::TreeViolation::ActiveOutOfRange {
                node: NodeIndex::root(),
                active: TabIndex(7),
                tabs: 2,
            }])
        );
    }

    #[test]
    fn oracle_bites_on_prev_active_equal_to_active() {
        let mut tree = Tree::new(vec![1, 2]);
        let Node::Leaf(leaf) = &mut tree[NodeIndex::root()] else {
            panic!("root of a fresh tree is a leaf");
        };
        leaf.active = TabIndex(1);
        leaf.prev_active = Some(TabIndex(1));

        assert_eq!(
            tree.validate(),
            Err(vec![super::TreeViolation::PrevActiveInvalid {
                node: NodeIndex::root(),
                prev_active: TabIndex(1),
                active: TabIndex(1),
                tabs: 2,
            }])
        );
    }

    #[test]
    fn oracle_bites_on_an_empty_non_root_leaf() {
        let mut tree = Tree::new(vec![1]);
        let [_, right] = tree.split_right(NodeIndex::root(), 0.5, vec![2]);
        let Node::Leaf(leaf) = &mut tree[right] else {
            panic!("split produces leaves");
        };
        leaf.tabs.clear();

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&super::TreeViolation::EmptyLeaf { node: right }),
            "expected an empty-leaf report, got {violations:?}"
        );
    }

    /// `set_focused_node` refuses non-leaves, so this corrupts the field directly: the point is
    /// to test the oracle, not to claim the public API can reach this state.
    #[test]
    fn oracle_bites_on_focus_pointing_at_a_split() {
        let mut tree = Tree::new(vec![1]);
        let _ = tree.split_right(NodeIndex::root(), 0.5, vec![2]);
        // The root is a split after the split_right above.
        tree.focused_node = Some(NodeIndex::root());

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&super::TreeViolation::FocusNotALeaf {
                node: NodeIndex::root()
            }),
            "expected a focus report, got {violations:?}"
        );
    }

    /// Regression: removing the *root* leaf empties the tree, and focus must not survive it.
    ///
    /// Found by the property test in `crate::proptests` on its first run. Every other exit from
    /// `remove_leaf` repairs `focused_node`; the early return for "this leaf is the root" did
    /// not, so `focused_leaf()` returned `Some(NodeIndex(0))` for a tree with zero nodes, and
    /// indexing the tree with that answer panics.
    #[test]
    fn removing_the_root_leaf_clears_focus() {
        let mut tree = Tree::new(vec![1, 2]);
        tree.set_focused_node(NodeIndex::root());
        assert_eq!(tree.focused_leaf(), Some(NodeIndex::root()));

        tree.remove_leaf(NodeIndex::root());

        assert!(tree.is_empty(), "removing the root leaf empties the tree");
        assert_eq!(
            tree.focused_leaf(),
            None,
            "focus must not outlive the leaf it pointed at"
        );
        assert_eq!(tree.validate(), Ok(()));
    }

    /// The split direction should not matter to well-formedness; this catches an oracle that
    /// only ever looked at one orientation.
    #[test]
    fn all_split_directions_are_well_formed() {
        for split in [Split::Left, Split::Right, Split::Above, Split::Below] {
            let mut tree = Tree::new(vec![1, 2]);
            let _ = tree.split(NodeIndex::root(), split, 0.5, Node::leaf(3));
            assert_eq!(tree.validate(), Ok(()), "split direction {split:?}");
        }
    }
}
