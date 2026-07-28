//! Property tests: random sequences of tree operations must keep the tree well-formed.
//!
//! The oracle is [`Tree::validate`](crate::Tree::validate). Unit tests pin down the cases
//! somebody thought of; these cover the ones nobody did, which is where the tree's positional
//! indices tend to bite — every structural operation renumbers nodes, so an operation that
//! forgets to shift an index produces a tree that still *type-checks* and still renders, just
//! with a subtree quietly detached or an active tab pointing at the wrong place.
//!
//! Two families of assertions:
//!
//! * **structure** — `validate()` after every single operation, so a failure names the operation
//!   that broke it rather than the end of the sequence;
//! * **conservation** — operations that are not supposed to destroy anything must not change the
//!   total number of tabs. Without this, a "well-formed" empty tree would pass happily.

use proptest::prelude::*;

use crate::{
    DockState, Node, NodeIndex, NodePath, Split, SurfaceIndex, TabIndex, TabInsert, TabPath, Tree,
};

/// One operation applied to the dock state.
///
/// Leaves are addressed as "the k-th live leaf" rather than by `NodeIndex`: node indices are
/// positions in an implicit heap and are renumbered by every split, so a generated `NodeIndex`
/// would mostly miss. `k` is taken modulo the number of live leaves at apply time.
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
        Just(Op::PushToFocused),
    ]
}

/// Live (non-`Empty`) leaves of the main surface, in slot order.
fn live_leaves<Tab>(tree: &Tree<Tab>) -> Vec<NodeIndex> {
    tree.iter()
        .enumerate()
        .filter_map(|(index, node)| matches!(node, Node::Leaf(_)).then_some(NodeIndex(index)))
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

/// Applies one operation. Returns `false` if it could not be applied at all (e.g. the tree has
/// no leaves left) — such a step is skipped rather than counted as a pass.
fn apply(dock_state: &mut DockState<u32>, op: Op, next_tab: &mut u32) -> bool {
    let main = SurfaceIndex::main();
    let leaves = live_leaves(dock_state.main_surface());
    if leaves.is_empty() {
        // Only `PushToFocused` can rebuild a tree from nothing.
        if let Op::PushToFocused = op {
            let tab = *next_tab;
            *next_tab += 1;
            dock_state.main_surface_mut().push_to_focused_leaf(tab);
            return true;
        }
        return false;
    }

    match op {
        Op::Split { leaf, split, tabs } => {
            let node = leaves[leaf % leaves.len()];
            let new_tabs: Vec<u32> = (0..tabs)
                .map(|_| {
                    let tab = *next_tab;
                    *next_tab += 1;
                    tab
                })
                .collect();
            let _ = dock_state.main_surface_mut().split(
                node,
                split_from(split),
                0.5,
                Node::leaf_with(new_tabs),
            );
        }

        Op::RemoveLeaf { leaf } => {
            let node = leaves[leaf % leaves.len()];
            dock_state.main_surface_mut().remove_leaf(node);
        }

        Op::RemoveTab { leaf, tab } => {
            let node = leaves[leaf % leaves.len()];
            let tab_count = dock_state.main_surface()[node].tabs_count();
            if tab_count == 0 {
                return false;
            }
            let _ = dock_state
                .main_surface_mut()
                .remove_tab((node, TabIndex(tab % tab_count)));
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
                return false;
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
        }

        Op::SetActive { leaf, tab } => {
            let node = leaves[leaf % leaves.len()];
            let tab_count = dock_state.main_surface()[node].tabs_count();
            if tab_count == 0 {
                return false;
            }
            let _ = dock_state
                .main_surface_mut()
                .set_active_tab(node, TabIndex(tab % tab_count));
        }

        Op::Focus { leaf } => {
            let node = leaves[leaf % leaves.len()];
            dock_state.main_surface_mut().set_focused_node(node);
        }

        Op::PushToFocused => {
            let tab = *next_tab;
            *next_tab += 1;
            dock_state.main_surface_mut().push_to_focused_leaf(tab);
        }
    }

    true
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
            if !apply(&mut dock_state, op, &mut next_tab) {
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
            if !apply(&mut dock_state, op, &mut next_tab) {
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
}
