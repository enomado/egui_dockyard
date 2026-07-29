//! Property tests: random sequences of dock operations must keep the dock well-formed.
//!
//! The oracle is [`DockState::validate`](crate::DockState::validate). Unit tests pin down the
//! cases somebody thought of; these cover the ones nobody did — historically the place where
//! the tree's positional indices bit, since every structural operation renumbered nodes and an
//! operation that forgot to shift an index produced a tree that still *type-checked* and still
//! rendered, just with a subtree quietly detached or an active tab pointing at the wrong place.
//!
//! The operations themselves live in [`crate::core::testkit`], shared with the fuzzer. They
//! used to be written down here a second time, and the second copy stayed on the **main
//! surface**: detaching a tab into a window, moving tabs out of one, closing a window — none of
//! that was reached by any property, though `validate()` has rules about exactly that
//! bookkeeping. Sharing the vocabulary closes the gap by construction rather than by somebody
//! remembering to add ops in two places.
//!
//! Four families of assertions:
//!
//! * **structure** — `validate()` after every single operation, so a failure names the operation
//!   that broke it rather than the end of the sequence;
//! * **conservation** — operations that are not supposed to destroy anything must not change the
//!   total number of tabs. Without this, a "well-formed" empty dock would pass happily;
//! * **identity** — a node id taken before an operation still names the same node afterwards,
//!   unless that operation was about that node. This is the property the arena exists for, and
//!   the one the heap representation could not have;
//! * **derived counts** — the collapsing numbers each split stores are what its subtree says.
//!
//! Plus a coverage sweep, which is not a property but a check on the *generator*: it asserts
//! that windows really do get opened, closed and moved between over a fixed set of seeds. A
//! sequence that never leaves the main surface would satisfy every property above for free.

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, TestRunner};

use std::collections::HashMap;

use crate::core::testkit::{Applied, Op, apply, check_tab_count, leaves, rebuilds_identities};
use crate::{DockState, NodePath, SurfaceIndex};

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u8..8, 0u8..4, 0u8..3).prop_map(|(leaf, split, tabs)| Op::Split { leaf, split, tabs }),
        (0u8..8).prop_map(|leaf| Op::RemoveLeaf { leaf }),
        (0u8..8, 0u8..4).prop_map(|(leaf, tab)| Op::RemoveTab { leaf, tab }),
        (0u8..8, 0u8..4, 0u8..8, 0u8..6).prop_map(|(src_leaf, src_tab, dst_leaf, insert)| {
            Op::MoveTab {
                src_leaf,
                src_tab,
                dst_leaf,
                insert,
            }
        }),
        (0u8..8, 0u8..4).prop_map(|(leaf, tab)| Op::SetActive { leaf, tab }),
        (0u8..8).prop_map(|leaf| Op::Focus { leaf }),
        (0u8..8).prop_map(|leaf| Op::ToggleCollapsed { leaf }),
        Just(Op::PushToFocused),
        (0u8..8, 0u8..4).prop_map(|(leaf, tab)| Op::Detach { leaf, tab }),
        (0u8..3).prop_map(|tabs| Op::AddWindow { tabs }),
        (0u8..4).prop_map(|window| Op::RemoveWindow { window }),
        Just(Op::RetainOdd),
        Just(Op::Remap),
        Just(Op::FilterOddCopying),
    ]
}

/// Tabs of every live leaf, keyed by identity. The snapshot the identity property compares.
fn leaf_contents(state: &DockState<u32>) -> HashMap<NodePath, Vec<u32>> {
    state
        .iter_leaves()
        .map(|(path, leaf)| (path, leaf.iter_tabs().copied().collect()))
        .collect()
}

/// Open windows, by index — the quantity the coverage sweep watches.
fn open_window_count(state: &DockState<u32>) -> usize {
    crate::core::testkit::open_windows(state).len()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// After every operation the dock must still be a dock.
    #[test]
    fn dock_invariants_hold(ops in prop::collection::vec(op_strategy(), 1..24)) {
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
    ///
    /// The count is over the *whole* dock, not the main surface: a detached tab leaves the main
    /// surface and stays in the dock, and a per-surface count would call that a loss.
    #[test]
    fn non_destructive_ops_conserve_tabs(ops in prop::collection::vec(op_strategy(), 1..24)) {
        let mut dock_state = DockState::new(vec![0u32, 1, 2]);
        let mut next_tab = 3u32;

        for (step, op) in ops.into_iter().enumerate() {
            let before = crate::core::testkit::total_tabs(&dock_state);
            if apply(&mut dock_state, op, &mut next_tab).is_none() {
                continue;
            }
            let after = crate::core::testkit::total_tabs(&dock_state);

            prop_assert!(
                check_tab_count(op, before, after).is_ok(),
                "step {}: {}",
                step,
                check_tab_count(op, before, after).unwrap_err()
            );
        }
    }

    /// Every collapsing number in the dock is what its subtree says it is.
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

            // Every surface, not just the main one: a detached tab carries its collapsed state
            // into the window it lands in, and the window reads its height off the tree's count.
            for (_, surface) in dock_state.iter_surfaces_indexed() {
                let Some(tree) = surface.node_tree() else { continue };

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
    ///
    /// Copying sweeps are skipped, and the reason is stated once in
    /// [`rebuilds_identities`]: they build a fresh arena, so a slot number may be handed out
    /// again to a different node — which is not the class this property is about.
    #[test]
    fn ids_keep_naming_the_same_node(ops in prop::collection::vec(op_strategy(), 1..24)) {
        let mut dock_state = DockState::new(vec![0u32, 1, 2]);
        let mut next_tab = 3u32;

        for (step, op) in ops.into_iter().enumerate() {
            let before = leaf_contents(&dock_state);
            let Some(Applied { touched }) = apply(&mut dock_state, op, &mut next_tab) else {
                continue;
            };
            if rebuilds_identities(op) {
                continue;
            }
            let after = leaf_contents(&dock_state);

            for (path, tabs) in &before {
                if touched.contains(path) {
                    continue;
                }
                if let Some(now) = after.get(path) {
                    prop_assert_eq!(
                        now, tabs,
                        "{:?} at step {} changed the tabs of an unrelated leaf {:?}", op, step, path
                    );
                }
            }
        }
    }
}

/// The generator must actually reach the window layer — otherwise every property above is
/// green for free.
///
/// This is not a property but a check on the *scene*: "0 violations" reads the same as
/// "0 checks", and until the operation vocabulary was shared with the fuzzer these tests
/// genuinely never left the main surface. Counting the outcomes is what makes the difference
/// visible; the counts are asserted, not printed.
///
/// Deterministic on purpose: a fixed set of seeds through proptest's own RNG, so a run that
/// fails the coverage bar fails it for everybody rather than once in a while.
#[test]
fn the_generator_reaches_the_window_layer() {
    let sequence = prop::collection::vec(op_strategy(), 24..25);

    let mut windows_opened = 0usize;
    let mut windows_closed = 0usize;
    let mut cross_surface_moves = 0usize;
    let mut tabs_lived_in_windows = 0usize;

    for seed in 0u64..32 {
        let mut runner = TestRunner::new_with_rng(
            Config::default(),
            proptest::test_runner::TestRng::from_seed(
                proptest::test_runner::RngAlgorithm::ChaCha,
                &seed.to_le_bytes().repeat(4),
            ),
        );
        let ops = sequence.new_tree(&mut runner).unwrap().current();

        let mut dock_state = DockState::new(vec![0u32, 1, 2]);
        let mut next_tab = 3u32;

        for op in ops {
            let windows_before = open_window_count(&dock_state);
            let Some(Applied { touched }) = apply(&mut dock_state, op, &mut next_tab) else {
                continue;
            };
            let windows_after = open_window_count(&dock_state);

            windows_opened += usize::from(windows_after > windows_before);
            windows_closed += usize::from(windows_after < windows_before);
            // A move whose two ends live on different surfaces — the path that has to keep the
            // surface bookkeeping straight while a tree changes underneath it.
            if matches!(op, Op::MoveTab { .. })
                && touched.len() == 2
                && touched[0].surface != touched[1].surface
            {
                cross_surface_moves += 1;
            }
            tabs_lived_in_windows += leaves(&dock_state)
                .iter()
                .filter(|path| path.surface != SurfaceIndex::Main)
                .count();
        }
    }

    assert!(
        windows_opened > 0,
        "no window was ever opened — the properties never left the main surface"
    );
    assert!(
        windows_closed > 0,
        "no window was ever closed — the surface bookkeeping of `remove_window` went unexercised"
    );
    assert!(
        cross_surface_moves > 0,
        "no tab was ever moved between surfaces — the cross-surface path went unexercised"
    );
    assert!(
        tabs_lived_in_windows > 0,
        "no leaf ever lived in a window — windows were opened and immediately gone"
    );
}
