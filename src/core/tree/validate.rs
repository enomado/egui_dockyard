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

use crate::core::SurfaceIndex;
use crate::core::tree::{Node, NodeId, TabIndex, Tree};

/// What is wrong with one entry of a leaf's focus history.
///
/// Carried by [`TreeViolation::FocusHistoryInvalid`], because "the history is broken" is three
/// different faults with three different causes, and a report that does not say which one sends
/// the reader back to the data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryProblem {
    /// The entry names a tab this leaf does not hold.
    NotInTheLeaf,
    /// The entry names the tab that is active, which is not somewhere to return *to*.
    IsActive,
    /// The entry appears earlier in the history as well.
    Repeated,
}

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

    /// A leaf that holds no tabs.
    ///
    /// Removing the last tab from a leaf is supposed to collapse the leaf
    /// ([`Tree::remove_tab`] does this); a surviving empty leaf means some path removed tabs
    /// without collapsing. The root is **not** exempt — an empty dock is a tree with no root
    /// at all (see [`Tree::new`]), so an empty leaf is a fault wherever it sits.
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

    /// An entry of the focus history names a tab that is not in the leaf, or the active one,
    /// or repeats an entry already in it.
    ///
    /// All three break the documented invariant, and each makes the fallback after removing
    /// the active tab do something other than what the history says: land on a tab that is
    /// gone, stay where it is, or hand out the same answer twice.
    FocusHistoryInvalid {
        /// The leaf node.
        node: NodeId,
        /// Where the offending entry sits, if it is present at all.
        entry: Option<TabIndex>,
        /// What is wrong with it.
        problem: HistoryProblem,
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

    /// A split's `fraction` is `NaN` or infinite.
    ///
    /// This is how a division by a zero-extent rectangle arrives: the layout pass multiplies the
    /// fraction by an extent, so a `NaN` here is a `NaN` rectangle, and a `NaN` rectangle fails
    /// every comparison it is put through — including the ones that decide which branch the
    /// renderer takes. The crate has been bitten by exactly this once.
    SplitFractionNotFinite {
        /// The split node. Its `fraction` is readable through [`Node::get_split`].
        node: NodeId,
    },

    /// A split's `fraction` is outside `0.0..=1.0`, so it is not a fraction of anything.
    ///
    /// Nothing in the crate can produce one: the only two writers are a drag, which clamps to
    /// what the geometry can honour, and a double-click, which writes `0.5`. It arrives from
    /// outside — a hand-built tree, a loaded layout, or an arithmetic slip in code that derives
    /// a fraction from measured pixels and does not ask whether the interval it measured can
    /// hold the answer.
    ///
    /// It does not crash: the renderer clamps at draw time. That is the reason to report it —
    /// the layout the tree describes and the layout on screen have quietly stopped being the
    /// same thing, and every later edit is made against the wrong one.
    SplitFractionOutOfRange {
        /// The split node. Its `fraction` is readable through [`Node::get_split`].
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

/// A single way in which a [`DockState`](crate::DockState) fails to be well-formed.
///
/// Most of them are about one surface's tree; the rest are about the state that ties the
/// surfaces together, which no per-tree check can see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DockViolation {
    /// The tree inside one of the surfaces is not well-formed.
    Tree(SurfaceViolation),

    /// `focused_surface` names a window that is gone.
    ///
    /// Windows are addressed by position, so closing one can leave this pointing past the end
    /// of the vector or at a hole — after which anything that resolves focus (pushing a tab to
    /// the focused leaf, for one) is reading a window that is not there.
    ///
    /// This rule used to have a sibling, `MainSurfaceMissing`. It is gone: the main surface is
    /// a field of [`DockState`](crate::DockState), so there is no state left for that rule to reject.
    FocusedSurfaceInvalid {
        /// The surface focus claims to be in.
        surface: SurfaceIndex,
    },
}

impl<Tab> Tree<Tab> {
    /// Checks the tree's structural invariants and returns every violation found.
    ///
    /// Read-only: nothing is mutated, nothing is repaired. An empty tree (no nodes at all) is
    /// well-formed — that is an empty dock, and the only shape one has. A leaf holding no
    /// tabs is not, wherever it sits; see [`TreeViolation::EmptyLeaf`].
    ///
    /// Intended as a test oracle rather than a runtime check — it walks every node, so it is
    /// linear in tree size and not meant for per-frame use.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use egui_dockyard::DockState;
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
                stack.extend(children.into_iter().flatten().copied().filter(|child| {
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
                        .is_some_and(|split| split.index_of(id).is_some());
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
                    for &child in split.children() {
                        if self.parent(child) != Some(id) {
                            violations.push(TreeViolation::ChildLinkBroken { node: id, child });
                        }
                    }
                    // The two faults are reported apart because they arrive from different
                    // places: a non-finite fraction is a division that should not have been
                    // done, an out-of-range one is arithmetic that answered outside the interval
                    // it was measuring. Only the first is checked first — `NaN` fails every
                    // comparison, so the range test would let it through.
                    if !split.fraction.is_finite() {
                        violations.push(TreeViolation::SplitFractionNotFinite { node: id });
                    } else if !(0.0..=1.0).contains(&split.fraction) {
                        violations.push(TreeViolation::SplitFractionOutOfRange { node: id });
                    }
                }

                Node::Leaf(leaf) => {
                    let tabs = leaf.len();
                    if tabs == 0 {
                        // No exemption for the root: an empty dock is a tree with *no root*,
                        // so a leaf holding no tabs is a fault wherever it sits.
                        violations.push(TreeViolation::EmptyLeaf { node: id });
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

                    let mut seen = Vec::new();
                    for entry in leaf.history_ids() {
                        let index = leaf.index_of(entry);
                        let problem = if index.is_none() {
                            Some(HistoryProblem::NotInTheLeaf)
                        } else if index == leaf.active_index() {
                            Some(HistoryProblem::IsActive)
                        } else if seen.contains(&entry) {
                            Some(HistoryProblem::Repeated)
                        } else {
                            None
                        };
                        seen.push(entry);
                        if let Some(problem) = problem {
                            violations.push(TreeViolation::FocusHistoryInvalid {
                                node: id,
                                entry: index,
                                problem,
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

impl<Tab> crate::core::DockState<Tab> {
    /// Runs [`Tree::validate`] over every surface, and checks the state that spans them.
    pub fn validate(&self) -> Result<(), Vec<DockViolation>> {
        let mut violations = Vec::new();

        for (surface_index, surface) in self.iter_surfaces_indexed() {
            let Some(tree) = surface.node_tree() else {
                continue;
            };
            if let Err(tree_violations) = tree.validate() {
                violations.extend(tree_violations.into_iter().map(|violation| {
                    DockViolation::Tree(SurfaceViolation {
                        surface: surface_index,
                        violation,
                    })
                }));
            }
        }

        // Focus is the one piece of state that outlives the surface it points into: closing a
        // window does not visit it, so it has to be checked from the outside.
        if let Some(surface) = self.focused_surface
            && !self.is_surface_valid(surface)
        {
            violations.push(DockViolation::FocusedSurfaceInvalid { surface });
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
    use super::{DockViolation, HistoryProblem, TreeViolation};
    use crate::core::tree::{Node, NodePath, Split, TabIndex, Tree};
    use crate::core::{DockState, SurfaceIndex};

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
    /// A fraction is a fraction: finite, and between the two ends of the interval it names.
    ///
    /// Neither fault crashes anything — the layout pass clamps at draw time — which is exactly
    /// why they need an oracle. Once one is stored, the tree and the screen describe different
    /// layouts, and every later edit is made against the wrong one. The `NaN` case has been real
    /// once (a division by a zero-extent rectangle) and the out-of-range case has been real
    /// once too, from test scaffolding that computed a fraction against one rectangle and wrote
    /// it into a node that had since been given a shorter one.
    ///
    /// The two are checked in that order in `validate`, and this pins the order: `NaN` fails
    /// every comparison including `>` and `<`, so a range test asked first would pass it.
    #[test]
    fn oracle_bites_on_a_fraction_that_is_not_one() {
        let build = |fraction: f32| {
            let mut tree = Tree::new(vec![1, 2]);
            let root = tree.root().unwrap();
            tree.split_right(root, 0.5, vec![3]);
            let split = tree.root().unwrap();
            tree[split].get_split_mut().unwrap().fraction = fraction;
            (tree.validate().unwrap_err(), split)
        };

        for bad in [1.003_f32, -0.2, 12.0] {
            let (violations, split) = build(bad);
            assert!(
                violations.contains(&TreeViolation::SplitFractionOutOfRange { node: split }),
                "a fraction of {bad} was accepted: {violations:?}"
            );
        }

        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let (violations, split) = build(bad);
            assert!(
                violations.contains(&TreeViolation::SplitFractionNotFinite { node: split }),
                "a fraction of {bad} was accepted, or was reported as merely out of range: \
                 {violations:?}"
            );
        }

        // And the ends of the interval are fractions. A split at 0.0 gives one child no room,
        // which the separator margin then takes back at draw time — a legitimate layout, saved
        // and loaded like any other, and not the oracle's business.
        for fine in [0.0_f32, 1.0, 0.5] {
            let mut tree = Tree::new(vec![1, 2]);
            let root = tree.root().unwrap();
            tree.split_right(root, 0.5, vec![3]);
            let split = tree.root().unwrap();
            tree[split].get_split_mut().unwrap().fraction = fine;
            assert_eq!(tree.validate(), Ok(()), "a fraction of {fine} was rejected");
        }
    }

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
        let index = tree[top].get_split().unwrap().index_of(middle).unwrap();
        tree[top].get_split_mut().unwrap().set_child(index, deep);

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
        let index = tree[inner].get_split().unwrap().index_of(deep).unwrap();
        tree[inner].get_split_mut().unwrap().set_child(index, split);

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
    fn oracle_bites_on_a_history_entry_that_is_the_active_tab() {
        let mut tree = Tree::new(vec![1, 2]);
        let root = tree.root().unwrap();
        tree.leaf_mut(root).unwrap().corrupt_prev_active_to_active();

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&TreeViolation::FocusHistoryInvalid {
                node: root,
                entry: Some(TabIndex(0)),
                problem: HistoryProblem::IsActive,
                active: Some(TabIndex(0)),
            }),
            "expected a focus-history report, got {violations:?}"
        );
    }

    /// The other shape of a broken history, and the one only a stack can have: an entry that
    /// is already in it. A duplicate makes the fallback hand out the same tab twice, so the
    /// second close after it lands on a tab that is gone.
    #[test]
    fn oracle_bites_on_a_repeated_history_entry() {
        let mut tree = Tree::new(vec![1, 2, 3]);
        let root = tree.root().unwrap();
        let leaf = tree.leaf_mut(root).unwrap();
        leaf.activate_tab_remembering(TabIndex(1));
        leaf.corrupt_history_with_a_duplicate();

        let violations = tree.validate().unwrap_err();
        assert!(
            violations.contains(&TreeViolation::FocusHistoryInvalid {
                node: root,
                entry: Some(TabIndex(0)),
                problem: HistoryProblem::Repeated,
                active: Some(TabIndex(1)),
            }),
            "expected a repeated-entry report, got {violations:?}"
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

    /// Focus that points into a surface which is not there must be *reported*, not left for
    /// whoever resolves it next to trip over.
    #[test]
    fn oracle_bites_on_focus_into_a_missing_surface() {
        let mut dock_state = DockState::new(vec![1, 2]);
        assert_eq!(dock_state.validate(), Ok(()));

        // No public call can leave focus dangling any more (that is the fix below), so the
        // field is corrupted directly: the point here is to test the oracle.
        dock_state.focused_surface = Some(SurfaceIndex::window(6));

        let violations = dock_state.validate().unwrap_err();
        assert!(
            violations.contains(&DockViolation::FocusedSurfaceInvalid {
                surface: SurfaceIndex::window(6),
            }),
            "expected a dangling-focus report, got {violations:?}"
        );
    }

    /// Regression, found by the `tree_ops` fuzz target: `retain_tabs` used to compact the
    /// surface vector, so a window emptied by the filter took the indices of every window
    /// after it down with it.
    ///
    /// The visible failure was a panic three operations later — `push_to_focused_leaf`
    /// resolving a `focused_surface` that now pointed past the end — but the quieter half is
    /// worse: a surviving window silently changed its `SurfaceIndex`, so anything holding one
    /// (a caller's handle, a saved layout's focus) started naming a different window.
    #[test]
    fn retain_tabs_does_not_renumber_surviving_windows() {
        let mut dock_state = DockState::new(vec![1]);
        let doomed = dock_state.add_window(vec![2]);
        let survivor = dock_state.add_window(vec![3]);
        assert_eq!(
            (doomed, survivor),
            (SurfaceIndex::window(0), SurfaceIndex::window(1))
        );

        // Drops the middle window entirely, keeps one tab on either side of it.
        dock_state.retain_tabs(|tab| *tab != 2);

        assert_eq!(dock_state.validate(), Ok(()));
        assert!(
            !dock_state.is_surface_valid(doomed),
            "the emptied window is gone"
        );
        assert!(
            dock_state.is_surface_valid(survivor),
            "the surviving window kept its index instead of sliding into the hole"
        );
        assert_eq!(
            dock_state[survivor].tabs().copied().collect::<Vec<_>>(),
            vec![3],
            "and it is still the same window, not a renumbered neighbour"
        );
    }

    /// Filtering every tab away must leave an empty dock, not a dock without a main surface.
    ///
    /// Found by the `tree_ops` fuzz target one run after the oracle learned to look at
    /// surfaces: the sweep nulled out any surface whose tree it emptied, and the main surface
    /// got the same treatment — after which `main_surface()` panicked and closing a window
    /// happily pointed focus back at a surface that was not there.
    ///
    /// This used to be guarded from two sides: the repair here, and a `MainSurfaceMissing`
    /// rule in the oracle to prove the repair was there. Both are gone, and their absence is
    /// the stronger statement — the main surface is a field of `DockState`, so there is no
    /// way left to write the state either of them was watching for. The test that corrupted a
    /// dock into that shape does not compile any more, which is why it is not below.
    #[test]
    fn retain_tabs_keeps_the_main_surface() {
        let mut dock_state = DockState::new(vec![1, 2]);
        dock_state.retain_tabs(|_| false);

        assert_eq!(dock_state.validate(), Ok(()));
        assert_eq!(
            dock_state.main_surface().num_tabs(),
            0,
            "an emptied dock still has a main surface, it just holds nothing"
        );
        assert_eq!(dock_state.focused_leaf(), None);
    }

    /// The panic itself: focus inside a window that `retain_tabs` empties, then a push.
    #[test]
    fn retain_tabs_that_drops_the_focused_window_leaves_focus_resolvable() {
        let mut dock_state = DockState::new(vec![1]);
        let window = dock_state.add_window(vec![2]);
        let root = dock_state[window].root().unwrap();
        dock_state.set_focused_node_and_surface(NodePath {
            surface: window,
            node: root,
        });

        dock_state.retain_tabs(|tab| *tab != 2);
        assert_eq!(dock_state.validate(), Ok(()));

        // Used to panic with "index out of bounds" inside `ensure_tree`.
        dock_state.push_to_focused_leaf(4);
        assert_eq!(dock_state.validate(), Ok(()));
        assert_eq!(
            dock_state[SurfaceIndex::main()].num_tabs(),
            2,
            "with nothing focused the tab lands in the main surface"
        );
    }

    /// The same thing end to end, the way a user reaches it: drag the only tab of a window
    /// onto an empty dock.
    ///
    /// There is no node to aim at — an empty dock is a tree with no root — so the drop arrives
    /// as [`TabDestination::EmptySurface`], which is exactly what the renderer offers over an
    /// empty surface. This scene used to aim at the empty root leaf and ask for a split; that
    /// leaf is gone, and with it the phantom pane a literal split would have made.
    #[test]
    fn dropping_a_tab_onto_an_empty_dock_leaves_no_phantom_pane() {
        use crate::core::tree::{TabDestination, TabPath};

        let mut dock_state = DockState::<u32>::new(vec![]);
        let window = dock_state.add_window(vec![1]);
        let source = dock_state[window].root().unwrap();
        assert!(
            dock_state.main_surface().root().is_none(),
            "an empty dock offers its whole area, not a leaf"
        );

        assert!(dock_state.move_tab(
            TabPath::new(window, source, TabIndex(0)),
            TabDestination::EmptySurface(SurfaceIndex::main()),
        ));

        assert_eq!(dock_state.validate(), Ok(()));
        assert_eq!(dock_state.main_surface().num_tabs(), 1);
        assert_eq!(
            dock_state.main_surface().len(),
            1,
            "the tab arrived as the whole dock, not as one half of a split"
        );
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
