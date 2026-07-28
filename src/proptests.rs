//! Property tests: random sequences of tree operations must keep the tree well-formed.
//!
//! The oracle is [`Tree::validate`](crate::Tree::validate). Unit tests pin down the cases
//! somebody thought of; these cover the ones nobody did — historically the place where the
//! tree's positional indices bit, since every structural operation renumbered nodes and an
//! operation that forgot to shift an index produced a tree that still *type-checked* and still
//! rendered, just with a subtree quietly detached or an active tab pointing at the wrong place.
//!
//! Three families of assertions:
//!
//! * **structure** — `validate()` after every single operation, so a failure names the operation
//!   that broke it rather than the end of the sequence;
//! * **conservation** — operations that are not supposed to destroy anything must not change the
//!   total number of tabs. Without this, a "well-formed" empty tree would pass happily;
//! * **identity** — a node id taken before an operation still names the same node afterwards,
//!   unless that operation was about that node. This is the property the arena exists for, and
//!   the one the heap representation could not have.

use proptest::prelude::*;

use std::collections::HashMap;

use crate::{
    DockState, Node, NodeId, NodePath, Split, SurfaceIndex, TabIndex, TabInsert, TabPath, Tree,
};

/// One operation applied to the dock state.
///
/// Leaves are addressed as "the k-th live leaf" rather than by id: ids cannot be generated out
/// of thin air (they are handed out by the arena), so the operation picks one at apply time.
/// `k` is taken modulo the number of live leaves.
#[derive(Clone, Copy, Debug)]
enum Op {
    Split {
        leaf: usize,
        split: usize,
        tabs: usize,
    },
    RemoveLeaf {
        leaf: usize,
    },
    RemoveTab {
        leaf: usize,
        tab: usize,
    },
    MoveTab {
        src_leaf: usize,
        src_tab: usize,
        dst_leaf: usize,
        insert: usize,
    },
    SetActive {
        leaf: usize,
        tab: usize,
    },
    Focus {
        leaf: usize,
    },
    /// The collapse button, spelled the way the tab bar spells it: flip the leaf, then let
    /// the tree settle the ancestors. Present so that the collapsing counts are exercised
    /// against a tree that is being reshaped underneath them — without it every count in
    /// every generated tree is zero, and a property about them proves nothing.
    ToggleCollapsed {
        leaf: usize,
    },
    PushToFocused,
}

/// Whether an operation is allowed to reduce the total number of tabs.
fn is_destructive(op: Op) -> bool {
    matches!(op, Op::RemoveLeaf { .. } | Op::RemoveTab { .. })
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0usize..8, 0usize..4, 1usize..4).prop_map(|(leaf, split, tabs)| Op::Split {
            leaf,
            split,
            tabs
        }),
        (0usize..8).prop_map(|leaf| Op::RemoveLeaf { leaf }),
        (0usize..8, 0usize..4).prop_map(|(leaf, tab)| Op::RemoveTab { leaf, tab }),
        (0usize..8, 0usize..4, 0usize..8, 0usize..6).prop_map(
            |(src_leaf, src_tab, dst_leaf, insert)| Op::MoveTab {
                src_leaf,
                src_tab,
                dst_leaf,
                insert
            }
        ),
        (0usize..8, 0usize..4).prop_map(|(leaf, tab)| Op::SetActive { leaf, tab }),
        (0usize..8).prop_map(|leaf| Op::Focus { leaf }),
        (0usize..8).prop_map(|leaf| Op::ToggleCollapsed { leaf }),
        Just(Op::PushToFocused),
    ]
}

/// Live leaves of the main surface, in tree order.
fn live_leaves<Tab>(tree: &Tree<Tab>) -> Vec<NodeId> {
    tree.breadth_first()
        .into_iter()
        .filter(|id| tree[*id].is_leaf())
        .collect()
}

/// Tabs of every live leaf, keyed by identity. The snapshot the identity property compares.
fn leaf_contents(tree: &Tree<u32>) -> HashMap<NodeId, Vec<u32>> {
    live_leaves(tree)
        .into_iter()
        .map(|id| (id, tree.leaf(id).unwrap().iter_tabs().copied().collect()))
        .collect()
}

fn split_from(index: usize) -> Split {
    match index % 4 {
        0 => Split::Left,
        1 => Split::Right,
        2 => Split::Above,
        _ => Split::Below,
    }
}

/// Applies one operation. Returns `None` if it could not be applied at all (e.g. the tree has
/// no leaves left) — such a step is skipped rather than counted as a pass. Otherwise returns
/// the leaves the operation was *about*, which is what the identity property excludes.
fn apply(dock_state: &mut DockState<u32>, op: Op, next_tab: &mut u32) -> Option<Vec<NodeId>> {
    let main = SurfaceIndex::main();
    let leaves = live_leaves(dock_state.main_surface());
    if leaves.is_empty() {
        // Only `PushToFocused` can rebuild a tree from nothing.
        if let Op::PushToFocused = op {
            let tab = *next_tab;
            *next_tab += 1;
            dock_state.main_surface_mut().push_to_focused_leaf(tab);
            return Some(
                dock_state
                    .main_surface()
                    .focused_leaf()
                    .into_iter()
                    .collect(),
            );
        }
        return None;
    }

    let touched = match op {
        Op::Split { leaf, split, tabs } => {
            let node = leaves[leaf % leaves.len()];
            let new_tabs: Vec<u32> = (0..tabs)
                .map(|_| {
                    let tab = *next_tab;
                    *next_tab += 1;
                    tab
                })
                .collect();
            let [_, new] = dock_state.main_surface_mut().split(
                node,
                split_from(split),
                0.5,
                Node::leaf_with(new_tabs),
            );
            vec![node, new]
        }

        Op::RemoveLeaf { leaf } => {
            let node = leaves[leaf % leaves.len()];
            dock_state.main_surface_mut().remove_leaf(node);
            vec![node]
        }

        Op::RemoveTab { leaf, tab } => {
            let node = leaves[leaf % leaves.len()];
            let tab_count = dock_state.main_surface()[node].tabs_count();
            if tab_count == 0 {
                return None;
            }
            let _ = dock_state
                .main_surface_mut()
                .remove_tab((node, TabIndex(tab % tab_count)));
            vec![node]
        }

        Op::MoveTab {
            src_leaf,
            src_tab,
            dst_leaf,
            insert,
        } => {
            let src_node = leaves[src_leaf % leaves.len()];
            let dst_node = leaves[dst_leaf % leaves.len()];
            let src_count = dock_state.main_surface()[src_node].tabs_count();
            let dst_count = dock_state.main_surface()[dst_node].tabs_count();
            if src_count == 0 {
                return None;
            }
            let src = TabPath::new(main, src_node, TabIndex(src_tab % src_count));
            let dst_path = NodePath {
                surface: main,
                node: dst_node,
            };
            // The insertion index is deliberately allowed to reach `dst_count` (append position)
            // and to be generated against the *pre-removal* count — that is exactly the
            // out-of-bounds case that had to be clamped in `move_tab`.
            let insert = match insert % 6 {
                0 => TabInsert::Append,
                1 => TabInsert::Insert(TabIndex(dst_count)),
                2 => TabInsert::Insert(TabIndex(dst_count.saturating_sub(1))),
                3 => TabInsert::Split(Split::Left),
                4 => TabInsert::Split(Split::Below),
                _ => TabInsert::Insert(TabIndex(0)),
            };
            dock_state.move_tab(src, (dst_path, insert));
            vec![src_node, dst_node]
        }

        Op::SetActive { leaf, tab } => {
            let node = leaves[leaf % leaves.len()];
            let tab_count = dock_state.main_surface()[node].tabs_count();
            if tab_count == 0 {
                return None;
            }
            let _ = dock_state
                .main_surface_mut()
                .set_active_tab(node, TabIndex(tab % tab_count));
            // Which tab is open does not change *which tabs are there*.
            vec![]
        }

        Op::Focus { leaf } => {
            let node = leaves[leaf % leaves.len()];
            dock_state.main_surface_mut().set_focused_node(node);
            vec![]
        }

        Op::ToggleCollapsed { leaf } => {
            let node = leaves[leaf % leaves.len()];
            let collapsed = dock_state.main_surface()[node].is_collapsed();
            dock_state.main_surface_mut()[node].set_collapsed(!collapsed);
            dock_state.main_surface_mut().node_update_collapsed(node);
            // Collapsing hides a leaf's tabs; it does not touch which tabs are where.
            vec![]
        }

        Op::PushToFocused => {
            let tab = *next_tab;
            *next_tab += 1;
            dock_state.main_surface_mut().push_to_focused_leaf(tab);
            dock_state
                .main_surface()
                .focused_leaf()
                .into_iter()
                .collect()
        }
    };

    Some(touched)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// After every operation the tree must still be a tree.
    #[test]
    fn tree_invariants_hold(ops in prop::collection::vec(op_strategy(), 1..24)) {
        let mut dock_state = DockState::new(vec![0u32, 1, 2]);
        let mut next_tab = 3u32;
        prop_assert_eq!(dock_state.validate(), Ok(()), "the initial state must be well-formed");

        for (step, op) in ops.into_iter().enumerate() {
            if apply(&mut dock_state, op, &mut next_tab).is_none() {
                continue;
            }
            prop_assert_eq!(
                dock_state.validate(),
                Ok(()),
                "violated after step {} ({:?})",
                step,
                op
            );
        }
    }

    /// Operations that are not supposed to destroy anything must not lose tabs.
    ///
    /// This is the half that a structural oracle alone cannot see: an implementation that
    /// "fixed" a broken move by dropping the tab would keep every structural invariant intact.
    #[test]
    fn non_destructive_ops_conserve_tabs(ops in prop::collection::vec(op_strategy(), 1..24)) {
        let mut dock_state = DockState::new(vec![0u32, 1, 2]);
        let mut next_tab = 3u32;

        for (step, op) in ops.into_iter().enumerate() {
            let before = dock_state.main_surface().num_tabs();
            if apply(&mut dock_state, op, &mut next_tab).is_none() {
                continue;
            }
            let after = dock_state.main_surface().num_tabs();

            match op {
                Op::Split { tabs, .. } => prop_assert_eq!(
                    after, before + tabs,
                    "split must add exactly the tabs it was given (step {})", step
                ),
                Op::PushToFocused => prop_assert_eq!(
                    after, before + 1,
                    "push must add exactly one tab (step {})", step
                ),
                op if !is_destructive(op) => prop_assert_eq!(
                    after, before,
                    "{:?} must not change the tab count (step {})", op, step
                ),
                _ => prop_assert!(
                    after <= before,
                    "a destructive op must not invent tabs (step {})", step
                ),
            }
        }
    }

    /// Every collapsing number in the tree is what its subtree says it is.
    ///
    /// The counts are *derived* — a split is collapsed exactly when both children are, and
    /// its row count is what the children stack up to — but they are stored, so every edit
    /// has to settle them. Historically each edit settled them in its own way, keyed to the
    /// gesture that happened rather than to the subtree that resulted, and three separate
    /// paths got it wrong (a removed leaf, a copying sweep, and a partially collapsed tree
    /// on the ordinary path). This says the same thing once, for whatever sequence of edits
    /// the generator comes up with.
    ///
    /// `validate()` deliberately does not check this: it is the oracle of *structure*, and a
    /// wrong row count renders a wrong height rather than a wrong tree.
    #[test]
    fn collapsed_counts_stay_derived(ops in prop::collection::vec(op_strategy(), 1..24)) {
        let mut dock_state = DockState::new(vec![0u32, 1, 2]);
        let mut next_tab = 3u32;
        let mut collapsed_seen = 0usize;

        for (step, op) in ops.into_iter().enumerate() {
            if apply(&mut dock_state, op, &mut next_tab).is_none() {
                continue;
            }
            let tree = dock_state.main_surface();

            for id in tree.breadth_first() {
                let Some(split) = tree[id].get_split() else { continue };
                let [left, right] = split.children();
                let expected_count = if tree[id].is_horizontal() {
                    tree[left].collapsed_leaf_count().max(tree[right].collapsed_leaf_count())
                } else {
                    tree[left].collapsed_leaf_count() + tree[right].collapsed_leaf_count()
                };
                prop_assert_eq!(
                    split.collapsed_leaf_count, expected_count,
                    "split {} carries a row count its children do not add up to, after step {} ({:?})",
                    id, step, op
                );
                prop_assert_eq!(
                    split.fully_collapsed,
                    tree[left].is_collapsed() && tree[right].is_collapsed(),
                    "split {} disagrees with its children about being collapsed, after step {} ({:?})",
                    id, step, op
                );
            }

            // And the tree mirrors its root — this is the number a floating window's height
            // is read from, and the one the ordinary collapse path used to skip.
            prop_assert_eq!(
                tree.collapsed_leaf_count(),
                tree.root_node().map_or(0, |root| root.collapsed_leaf_count()),
                "the tree disagrees with its root, after step {} ({:?})", step, op
            );

            collapsed_seen += usize::from(tree.collapsed_leaf_count() > 0);
        }

        // Guards against the property passing on a scene where nothing is ever collapsed:
        // every count would be zero and every assertion above would hold for free. The
        // sequence is random, so this only demands that *some* run reaches a collapsed tree —
        // proptest's shrinking would otherwise happily report a green run over 24 no-ops.
        prop_assume!(collapsed_seen > 0);
    }

    /// A node id keeps naming the same node across operations that are not about it.
    ///
    /// This is the property the whole arena exists for, and the one the previous
    /// representation could not satisfy: there, a split renumbered every node after the
    /// split point, so an id held across it addressed a different node — silently, and only
    /// sometimes, which is why the two bugs it caused took so long to pin down.
    #[test]
    fn ids_keep_naming_the_same_node(ops in prop::collection::vec(op_strategy(), 1..24)) {
        let mut dock_state = DockState::new(vec![0u32, 1, 2]);
        let mut next_tab = 3u32;

        for (step, op) in ops.into_iter().enumerate() {
            let before = leaf_contents(dock_state.main_surface());
            let Some(touched) = apply(&mut dock_state, op, &mut next_tab) else {
                continue;
            };
            let after = leaf_contents(dock_state.main_surface());

            for (id, tabs) in &before {
                if touched.contains(id) {
                    continue;
                }
                if let Some(now) = after.get(id) {
                    prop_assert_eq!(
                        now, tabs,
                        "{:?} at step {} changed the tabs of an unrelated leaf {}", op, step, id
                    );
                }
            }
        }
    }
}
