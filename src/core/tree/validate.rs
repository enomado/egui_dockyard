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
//! Nodes live in an arena and the shape is carried by explicit links: a child knows its
//! parent, a split knows its two children. Nothing forces those links to agree — so the
//! checks below are what make the arena a *tree*:
//!
//! * both link directions agree (which is also what makes "exactly one parent" hold: a
//!   node has one parent field, so a second split claiming it would disagree with it);
//! * every node is reachable from the root, and reachable exactly once — no orphans, no
//!   cycles;
//! * a leaf's focus state names tabs that are actually there.
//!
//! The arena moved a whole class of these from "checked" to "impossible": a split cannot
//! have one child, because it stores both, and a stale id cannot silently name a different
//! node, because the generation stops matching. What remains checkable is checked here.
//!
//! # What is *not* being checked
//!
//! Geometry (`rect`/`viewport`) is deliberately ignored: it is a per-frame cache written by the
//! layout pass, not state, so it is meaningless before the first frame and stale afterwards.

use std::collections::HashSet;

use crate::{Node, NodeId, Surface, SurfaceIndex, TabIndex, Tree};

/// A single way in which a [`Tree`] fails to be well-formed.
///
/// Reported by [`Tree::validate`]. Each variant carries enough context to point at the offending
/// node, so a failing property test or fuzz case names the place rather than just "invalid".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TreeViolation {
    /// The arena holds nodes but no root is set, so nothing is reachable at all.
    RootMissing,

    /// The root node claims to have a parent.
    RootHasParent {
        /// The root.
        node: NodeId,
        /// What it claims as its parent.
        parent: NodeId,
    },

    /// A node that is not reachable from the root.
    ///
    /// No traversal will ever visit it, but it still owns its tabs, so this is how tabs go
    /// missing while remaining in memory.
    OrphanNode {
        /// The unreachable node.
        node: NodeId,
    },

    /// A node reached twice while walking down from the root: the links form a cycle (or a
    /// diamond), not a tree.
    CycleDetected {
        /// The node reached for the second time.
        node: NodeId,
    },

    /// A node whose parent link does not resolve, points at a leaf, or points at a split
    /// that does not list it as a child.
    ParentLinkBroken {
        /// The node with the bad link.
        node: NodeId,
        /// What it claims as its parent.
        parent: NodeId,
    },

    /// A split whose child id does not resolve, or whose child disagrees about who its
    /// parent is.
    ChildLinkBroken {
        /// The split node.
        node: NodeId,
        /// The child it claims.
        child: NodeId,
    },

    /// A non-root leaf that holds no tabs.
    ///
    /// Removing the last tab from a leaf is supposed to collapse the leaf
    /// ([`Tree::remove_tab`] does this); a surviving empty leaf means some path removed tabs
    /// without collapsing. The root leaf is exempt: an empty dock is a legitimate state.
    EmptyLeaf {
        /// The empty leaf.
        node: NodeId,
    },

    /// A leaf whose active tab is not one of its tabs, or which has tabs but no active one.
    ///
    /// The invariant is "active is `Some` exactly when the leaf has tabs".
    ActiveInvalid {
        /// The leaf node.
        node: NodeId,
        /// Where the active tab sits, if it is present at all.
        active: Option<TabIndex>,
        /// How many tabs the leaf actually has.
        tabs: usize,
    },

    /// `prev_active` names a tab that is not in the leaf, or the active one.
    ///
    /// Both cases break the documented invariant: falling back to it after removing the
    /// active tab would then either do nothing or return to the tab just removed.
    PrevActiveInvalid {
        /// The leaf node.
        node: NodeId,
        /// Where the remembered tab sits, if it is present at all.
        prev_active: Option<TabIndex>,
        /// Where the active tab sits, for context.
        active: Option<TabIndex>,
    },

    /// `focused_node` points at a node that is gone or is not a leaf.
    ///
    /// Focus is what `push_to_focused_leaf` targets, so a stale focus silently redirects newly
    /// opened tabs.
    FocusNotALeaf {
        /// The offending focus target.
        node: NodeId,
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
    /// # use egui_dock::DockState;
    /// let mut dock_state = DockState::new(vec!["tab 1", "tab 2"]);
    /// let root = dock_state.main_surface().root().unwrap();
    /// let _ = dock_state.main_surface_mut().split_left(root, 0.5, vec!["tab 3"]);
    /// assert_eq!(dock_state.main_surface().validate(), Ok(()));
    /// ```
    pub fn validate(&self) -> Result<(), Vec<TreeViolation>> {
        let mut violations = Vec::new();

        // 1. Walk down from the root, guarding against cycles. This is deliberately not
        //    `breadth_first()`: that helper trusts the links, and would spin forever on a
        //    cycle instead of reporting one.
        let mut reachable = HashSet::new();
        let mut stack: Vec<NodeId> = self.root().into_iter().collect();
        while let Some(id) = stack.pop() {
            if !reachable.insert(id) {
                violations.push(TreeViolation::CycleDetected { node: id });
                continue;
            }
            if let Ok(children) = self
                .node(id)
                .map(|node| node.get_split().map(|s| s.children()))
            {
                stack.extend(children.into_iter().flatten().filter(|child| {
                    // Broken child links are reported below; do not walk into them.
                    self.contains(*child)
                }));
            }
        }

        // Note: `is_empty()` asks about the *root*, so it cannot answer this — the case
        // here is precisely an arena holding nodes that no root points at.
        if self.root().is_none() && self.iter().next().is_some() {
            violations.push(TreeViolation::RootMissing);
        }

        // 2. Per-node checks over the arena, so that nodes the walk never reached are seen
        //    too — an orphan is exactly a node the arena knows and the walk does not.
        for (id, node) in self.iter_indexed() {
            if !reachable.contains(&id) {
                violations.push(TreeViolation::OrphanNode { node: id });
            }

            match self.parent(id) {
                Some(parent) => {
                    let claims_it = self
                        .node(parent)
                        .ok()
                        .and_then(Node::get_split)
                        .is_some_and(|split| split.side_of(id).is_some());
                    if !claims_it {
                        violations.push(TreeViolation::ParentLinkBroken { node: id, parent });
                    }
                }
                None if Some(id) != self.root() => {
                    // A parentless node that is not the root is unreachable; already
                    // reported as an orphan above.
                }
                None => {}
            }

            match node {
                Node::Vertical(split) | Node::Horizontal(split) => {
                    for child in split.children() {
                        if self.parent(child) != Some(id) {
                            violations.push(TreeViolation::ChildLinkBroken { node: id, child });
                        }
                    }
                }

                Node::Leaf(leaf) => {
                    let tabs = leaf.len();
                    if tabs == 0 {
                        // The root leaf is allowed to be empty: that is just an empty dock.
                        if Some(id) != self.root() {
                            violations.push(TreeViolation::EmptyLeaf { node: id });
                        }
                        if leaf.active_id().is_some() {
                            violations.push(TreeViolation::ActiveInvalid {
                                node: id,
                                active: leaf.active_index(),
                                tabs,
                            });
                        }
                    } else if leaf.active_index().is_none() {
                        // Either no active tab at all, or one naming a tab that is gone.
                        violations.push(TreeViolation::ActiveInvalid {
                            node: id,
                            active: None,
                            tabs,
                        });
                    }

                    if let Some(prev_active) = leaf.prev_active_id() {
                        let index = leaf.index_of(prev_active);
                        if index.is_none() || index == leaf.active_index() {
                            violations.push(TreeViolation::PrevActiveInvalid {
                                node: id,
                                prev_active: index,
                                active: leaf.active_index(),
                            });
                        }
                    }
                }
            }
        }

        // 3. The root is the one node allowed to have no parent, and required to have none.
        if let Some(root) = self.root()
            && let Some(parent) = self.parent(root)
        {
            violations.push(TreeViolation::RootHasParent { node: root, parent });
        }

        if let Some(focused) = self.focused_leaf()
            && !self.node(focused).is_ok_and(Node::is_leaf)
        {
            violations.push(TreeViolation::FocusNotALeaf { node: focused });
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
    use super::TreeViolation;
    use crate::{DockState, Node, Split, TabIndex, Tree};

    /// The oracle must accept trees built through the public API — otherwise every later test
    /// that uses it is just measuring the oracle's own false positives.
    #[test]
    fn well_formed_trees_pass() {
        assert_eq!(Tree::<i32>::new(vec![]).validate(), Ok(()));
        assert_eq!(Tree::new(vec![1, 2, 3]).validate(), Ok(()));

        let mut dock_state = DockState::new(vec![1, 2]);
        let root = dock_state.main_surface().root().unwrap();
        let [_, right] = dock_state
            .main_surface_mut()
            .split_right(root, 0.5, vec![3]);
        let _ = dock_state
            .main_surface_mut()
            .split_below(right, 0.5, vec![4]);
        assert_eq!(dock_state.validate(), Ok(()));
        assert_eq!(dock_state.main_surface().validate(), Ok(()));
    }

    /// Each of these corrupts a tree in one specific way and asserts that the oracle names that
    /// specific way. Without them, `validate()` returning `Ok` would prove nothing: an oracle
    /// that never bites is indistinguishable from no oracle at all.
    ///
    /// Note what it takes to corrupt an arena tree: the public API cannot produce any of
    /// these states, so each test reaches into the private links directly. That is the
    /// point — the states that used to be reachable by ordinary use are gone.
    #[test]
    fn oracle_bites_on_a_dangling_child() {
        let mut tree = Tree::new(vec![1, 2]);
        let root = tree.root().unwrap();
        let [_, right] = tree.split_right(root, 0.5, vec![3]);
        let split = tree.root().unwrap();

        // Drop the right child out of the arena behind the split's back.
        tree.nodes.remove(right).unwrap();

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&TreeViolation::ChildLinkBroken {
                node: split,
                child: right,
            }),
            "expected a broken child link, got {violations:?}"
        );
    }

    #[test]
    fn oracle_bites_on_an_orphan() {
        let mut tree = Tree::new(vec![1]);
        let root = tree.root().unwrap();
        let [left, _right] = tree.split_right(root, 0.5, vec![2]);
        let [_, deep] = tree.split_right(left, 0.5, vec![3]);
        assert_eq!(tree.validate(), Ok(()), "precondition: tree is well-formed");

        // Cut the middle split loose: everything under it is now unreachable.
        let middle = tree.parent(deep).unwrap();
        let top = tree.parent(middle).unwrap();
        let side = tree[top].get_split().unwrap().side_of(middle).unwrap();
        tree[top].get_split_mut().unwrap().set_child(side, deep);

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&TreeViolation::OrphanNode { node: left }),
            "expected the cut-off subtree reported as orphaned, got {violations:?}"
        );
    }

    /// A cycle must be *reported*, not hung on. The walk that reports it is the reason
    /// `validate` does not reuse `breadth_first`.
    #[test]
    fn oracle_bites_on_a_cycle_instead_of_looping() {
        let mut tree = Tree::new(vec![1]);
        let root = tree.root().unwrap();
        let [left, right] = tree.split_right(root, 0.5, vec![2]);
        let split = tree.root().unwrap();
        let [_, deep] = tree.split_below(right, 0.5, vec![3]);
        let inner = tree.parent(deep).unwrap();

        // Point the inner split's own child back at the outer split.
        let side = tree[inner].get_split().unwrap().side_of(deep).unwrap();
        tree[inner].get_split_mut().unwrap().set_child(side, split);

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&TreeViolation::CycleDetected { node: split }),
            "expected a cycle report, got {violations:?}"
        );
        let _ = left;
    }

    #[test]
    fn oracle_bites_on_a_broken_parent_link() {
        let mut tree = Tree::new(vec![1]);
        let root = tree.root().unwrap();
        let [left, right] = tree.split_right(root, 0.5, vec![2]);

        // Tell the left leaf that its parent is its sibling.
        tree.nodes.get_mut(left).unwrap().parent = Some(right);

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&TreeViolation::ParentLinkBroken {
                node: left,
                parent: right,
            }),
            "expected a broken parent link, got {violations:?}"
        );
    }

    #[test]
    fn oracle_bites_on_an_out_of_range_active_tab() {
        let mut tree = Tree::new(vec![1, 2]);
        let root = tree.root().unwrap();
        // Drop the tabs behind the leaf's back so that `active` names a tab that is gone.
        let leaf = tree.leaf_mut(root).unwrap();
        let active = leaf.active_index().unwrap();
        leaf.corrupt_clear_tabs();

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&TreeViolation::ActiveInvalid {
                node: root,
                active: None,
                tabs: 0,
            }),
            "expected an active-tab report, got {violations:?} (was {active:?})"
        );
    }

    #[test]
    fn oracle_bites_on_prev_active_equal_to_active() {
        let mut tree = Tree::new(vec![1, 2]);
        let root = tree.root().unwrap();
        tree.leaf_mut(root).unwrap().corrupt_prev_active_to_active();

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&TreeViolation::PrevActiveInvalid {
                node: root,
                prev_active: Some(TabIndex(0)),
                active: Some(TabIndex(0)),
            }),
            "expected a prev_active report, got {violations:?}"
        );
    }

    #[test]
    fn oracle_bites_on_an_empty_non_root_leaf() {
        let mut tree = Tree::new(vec![1]);
        let root = tree.root().unwrap();
        let [_, right] = tree.split_right(root, 0.5, vec![2]);
        tree.leaf_mut(right).unwrap().corrupt_clear_tabs();

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&TreeViolation::EmptyLeaf { node: right }),
            "expected an empty-leaf report, got {violations:?}"
        );
    }

    /// `set_focused_node` refuses non-leaves, so this corrupts the field directly: the point is
    /// to test the oracle, not to claim the public API can reach this state.
    #[test]
    fn oracle_bites_on_focus_pointing_at_a_split() {
        let mut tree = Tree::new(vec![1]);
        let root = tree.root().unwrap();
        let _ = tree.split_right(root, 0.5, vec![2]);
        let split = tree.root().unwrap();
        tree.focused_node = Some(split);

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&TreeViolation::FocusNotALeaf { node: split }),
            "expected a focus report, got {violations:?}"
        );
    }

    /// Regression: removing the *root* leaf empties the tree, and focus must not survive it.
    ///
    /// Found by the property test in `crate::proptests` on its first run against the heap
    /// representation: the early return for "this leaf is the root" did not clear focus, so
    /// `focused_leaf()` kept naming a node that no longer existed.
    #[test]
    fn removing_the_root_leaf_clears_focus() {
        let mut tree = Tree::new(vec![1, 2]);
        let root = tree.root().unwrap();
        tree.set_focused_node(root);
        assert_eq!(tree.focused_leaf(), Some(root));

        tree.remove_leaf(root);

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
            let root = tree.root().unwrap();
            let _ = tree.split(root, split, 0.5, Node::leaf(3));
            assert_eq!(tree.validate(), Ok(()), "split direction {split:?}");
        }
    }
}
