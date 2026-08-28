//! One vocabulary of dock operations, shared by the property tests and the fuzzer.
//!
//! # Why this module exists
//!
//! The operations, the way a target is picked, and the oracle that says how many tabs an
//! operation may add or drop were written down **twice**: once in `src/proptests.rs` and once
//! in `fuzz/fuzz_targets/tree_ops.rs`. The two copies drifted exactly the way copies do, and
//! the drift was not cosmetic — it decided what got tested:
//!
//! * the property tests never left the **main surface**. Detaching a tab into a window,
//!   moving tabs back out of one, closing a window — none of it was reached by any property,
//!   although `validate()` has rules about surface bookkeeping and P0 wrote the gap down as
//!   "extend when the arena reaches the surface layer";
//! * the fuzzer, in turn, never checked **identity** or the **collapsing counts**, because
//!   those properties live on the property-test side;
//! * the conservation oracle was stated twice with different arithmetic (`tabs` against
//!   `tabs % 3 + 1`), so "the same operation" added a different number of tabs depending on
//!   which harness ran it.
//!
//! A rule that is re-derived at every call site is the shape this track has already been
//! bitten by twice — the two surface sweeps of P3 and the dock-shape dump of P11. So the
//! vocabulary lives here once: the generators stay on their own sides (proptest draws blind,
//! libFuzzer draws with coverage feedback), and *what an operation is* does not.
//!
//! # What stays outside
//!
//! Generators. `proptest` needs a `Strategy`, `libFuzzer` needs `Arbitrary`, and neither is a
//! property of the operation itself. Only the `Arbitrary` derive is here, behind the
//! `testkit` feature, because a foreign trait cannot be implemented on this type from the
//! fuzzer crate.

use crate::core::DockState;
use crate::core::geom::{Point, Rect, Size};
use crate::core::surface_index::{SurfaceIndex, WindowIndex};
use crate::core::tree::node::Node;
use crate::core::tree::{NodePath, Split, TabIndex, TabInsert, TabPath};

/// One operation applied to a dock state.
///
/// Nodes are addressed as "the k-th live leaf", never by id: ids are handed out by the arena
/// and cannot be invented by a generator, so an operation picks its target at apply time and
/// `k` is taken modulo the number of live leaves. Surfaces are picked the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "testkit", derive(arbitrary::Arbitrary))]
pub enum Op {
    /// Split a leaf, putting fresh tabs in the new half.
    Split {
        /// Which live leaf to split.
        leaf: u8,
        /// Which of the four directions.
        split: u8,
        /// How many tabs the new half gets (1..=3).
        tabs: u8,
    },
    /// Remove a whole leaf.
    RemoveLeaf {
        /// Which live leaf.
        leaf: u8,
    },
    /// Remove one tab of a leaf.
    RemoveTab {
        /// Which live leaf.
        leaf: u8,
        /// Which tab of it.
        tab: u8,
    },
    /// Move a tab somewhere else — possibly onto another surface.
    MoveTab {
        /// Leaf the tab is taken from.
        src_leaf: u8,
        /// Tab within that leaf.
        src_tab: u8,
        /// Leaf the tab is dropped on.
        dst_leaf: u8,
        /// Which destination shape (append, insert at an index, split).
        insert: u8,
    },
    /// Open another tab of a leaf.
    SetActive {
        /// Which live leaf.
        leaf: u8,
        /// Which tab of it.
        tab: u8,
    },
    /// Move the focus to a leaf.
    Focus {
        /// Which live leaf.
        leaf: u8,
    },
    /// The collapse button, spelled the way the tab bar spells it.
    ///
    /// Present so that the collapsing counts are exercised against a tree that is being
    /// reshaped underneath them — without it every count in every generated tree is zero and
    /// a property about them holds for free.
    ToggleCollapsed {
        /// Which live leaf.
        leaf: u8,
    },
    /// Put a whole split away behind one arrow, or bring it back — the Shift half of the
    /// collapse button.
    ///
    /// A separate op rather than a flag on the one above, because it is a decision about a
    /// *subtree* and touches nothing inside it: the two reach different states, and a property
    /// about the row count of a stowed split cannot be reached by collapsing leaves at all.
    ToggleStowed {
        /// Which live split.
        split: u8,
    },
    /// Push a tab to the focused leaf. One of the two ops that can rebuild an emptied dock.
    PushToFocused,
    /// Pull a tab out into its own window — the gesture that creates a surface by hand.
    Detach {
        /// Leaf the tab is taken from.
        leaf: u8,
        /// Tab within that leaf.
        tab: u8,
    },
    /// Open a window with fresh tabs.
    AddWindow {
        /// How many tabs it gets (1..=3).
        tabs: u8,
    },
    /// Close a whole window. The main surface is never a candidate — it has no
    /// [`WindowIndex`] to name it by.
    RemoveWindow {
        /// Which open window.
        window: u8,
    },
    /// Drop every tab whose value is odd — a destructive sweep that touches every surface at
    /// once, which no other op does.
    RetainOdd,
    /// Rebuild the whole dock through the copying sweep (`map_tabs`) with an identity
    /// function. Structurally the widest operation there is — every surface and every node is
    /// rebuilt — and it must still be invisible: same tabs, same surfaces, same indices.
    Remap,
    /// The copying sibling of [`Op::RetainOdd`]: the same sweep through `filter_tabs`, which
    /// builds a new dock instead of editing this one. The two paths keep the surface vector in
    /// different code, so a fix to one says nothing about the other.
    FilterOddCopying,
}

/// How many tabs an operation is allowed to add or take away.
///
/// Stated once, here, because the two harnesses used to state it separately and disagreed
/// about the arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabCountRule {
    /// Adds exactly this many tabs and takes none away.
    Adds(usize),
    /// Must leave the total alone.
    Unchanged,
    /// May drop tabs, must not invent any.
    MayShrink,
}

/// The number of tabs [`Op::Split`], [`Op::AddWindow`] and friends put in.
///
/// Kept next to [`apply`] rather than in each harness: the count is decided when the tabs are
/// created, and a second statement of it is a second thing to keep in sync.
fn fresh_tab_count(tabs: u8) -> usize {
    usize::from(tabs % 3) + 1
}

/// What the tab total must do across `op`.
pub fn tab_count_rule(op: Op) -> TabCountRule {
    match op {
        Op::Split { tabs, .. } | Op::AddWindow { tabs } => {
            TabCountRule::Adds(fresh_tab_count(tabs))
        }
        Op::PushToFocused => TabCountRule::Adds(1),
        Op::RemoveLeaf { .. }
        | Op::RemoveTab { .. }
        | Op::RemoveWindow { .. }
        | Op::RetainOdd
        | Op::FilterOddCopying => TabCountRule::MayShrink,
        // Detaching moves a tab into a window of its own — it leaves the surface it was on,
        // and stays in the dock. A per-surface count would call that a loss, which is why the
        // conservation oracle counts the whole dock.
        Op::Detach { .. }
        | Op::MoveTab { .. }
        | Op::SetActive { .. }
        | Op::Focus { .. }
        | Op::ToggleCollapsed { .. }
        | Op::ToggleStowed { .. }
        | Op::Remap => TabCountRule::Unchanged,
    }
}

/// Judges the tab total across one applied operation.
///
/// Returns the complaint as a string so that both harnesses can wrap it their own way
/// (`panic!` in the fuzzer, `prop_assert!` in the property tests).
pub fn check_tab_count(op: Op, before: usize, after: usize) -> Result<(), String> {
    match tab_count_rule(op) {
        TabCountRule::Adds(added) if after != before + added => Err(format!(
            "{op:?} had to add exactly {added} tab(s): {before} → {after}"
        )),
        TabCountRule::Unchanged if after != before => Err(format!(
            "{op:?} must not change the tab count: {before} → {after}"
        )),
        TabCountRule::MayShrink if after > before => Err(format!(
            "{op:?} is destructive but invented tabs: {before} → {after}"
        )),
        _ => Ok(()),
    }
}

/// Whether the operation hands out fresh node identities for the whole dock.
///
/// The copying sweeps build a new arena, so ids start from scratch — an id taken before such
/// a step may name a *different* node afterwards purely because the slot number was reused.
/// Any property about identity has to skip these steps rather than report them; that is a
/// property of the operation, so it is stated here and not in the harness that trips over it.
pub fn rebuilds_identities(op: Op) -> bool {
    matches!(op, Op::Remap | Op::FilterOddCopying)
}

/// Turns a generated number into one of the four split directions.
pub fn split_from(index: u8) -> Split {
    match index % 4 {
        0 => Split::Left,
        1 => Split::Right,
        2 => Split::Above,
        _ => Split::Below,
    }
}

/// Every leaf of every live surface, in a stable order.
pub fn leaves<Tab>(state: &DockState<Tab>) -> Vec<NodePath> {
    state.iter_leaves().map(|(path, _)| path).collect()
}

/// Total number of tabs held anywhere in the dock.
pub fn total_tabs<Tab>(state: &DockState<Tab>) -> usize {
    state.iter_all_tabs().count()
}

/// Windows that exist and hold something — the ones that may legally be closed.
pub fn open_windows<Tab>(state: &DockState<Tab>) -> Vec<WindowIndex> {
    state
        .iter_surfaces_indexed()
        // The main surface filters itself out: it has no `WindowIndex` to offer, so it cannot
        // even be a candidate for closing.
        .filter_map(|(index, surface)| (!surface.is_empty()).then(|| index.as_window()).flatten())
        .collect()
}

/// What one applied operation did, as far as the oracles need to know.
#[derive(Debug, Clone)]
pub struct Applied {
    /// The leaves the operation was *about* — the ones an identity property must exclude,
    /// since it is their contents the operation was allowed to change.
    pub touched: Vec<NodePath>,
}

/// Applies one operation, or reports that it was not applicable at all.
///
/// "Not applicable" (`None`) is a real state, not a failure: a sequence can empty the dock out,
/// and an op that needs a tab to work on then has nothing to do. Such a step is skipped rather
/// than counted, so no oracle is asked about an operation that never ran.
///
/// The tab type is `u32` on purpose: the sweeps select tabs by parity, and a fresh tab has to
/// be generatable without a factory. `next_tab` is the counter they come from — every tab in a
/// run is distinct, which is what makes the conservation and identity oracles readable.
pub fn apply(state: &mut DockState<u32>, op: Op, next_tab: &mut u32) -> Option<Applied> {
    let mut fresh_tab = || {
        let tab = *next_tab;
        *next_tab += 1;
        tab
    };
    let applied = |touched: Vec<NodePath>| Some(Applied { touched });

    let live = leaves(state);
    if live.is_empty() {
        // Only these two can rebuild a dock from nothing.
        return match op {
            Op::PushToFocused => {
                state.push_to_focused_leaf(fresh_tab());
                applied(push_landing(state))
            }
            Op::AddWindow { tabs } => {
                let tabs: Vec<u32> = (0..fresh_tab_count(tabs)).map(|_| fresh_tab()).collect();
                let surface = state.add_window(tabs);
                applied(leaves_of(state, surface))
            }
            _ => None,
        };
    }
    let pick = |k: u8| live[usize::from(k) % live.len()];

    let touched = match op {
        Op::Split { leaf, split, tabs } => {
            let path = pick(leaf);
            let tabs: Vec<u32> = (0..fresh_tab_count(tabs)).map(|_| fresh_tab()).collect();
            let [_, new] = state.split(path, split_from(split), 0.5, Node::leaf_with(tabs));
            vec![
                path,
                NodePath {
                    surface: path.surface,
                    node: new,
                },
            ]
        }

        Op::RemoveLeaf { leaf } => {
            let path = pick(leaf);
            state.remove_leaf(path);
            vec![path]
        }

        Op::RemoveTab { leaf, tab } => {
            let path = pick(leaf);
            let count = state[path].tabs_count();
            if count == 0 {
                return None;
            }
            state.remove_tab(TabPath::new(
                path.surface,
                path.node,
                TabIndex(usize::from(tab) % count),
            ));
            vec![path]
        }

        Op::MoveTab {
            src_leaf,
            src_tab,
            dst_leaf,
            insert,
        } => {
            let src_path = pick(src_leaf);
            let dst_path = pick(dst_leaf);
            let src_count = state[src_path].tabs_count();
            let dst_count = state[dst_path].tabs_count();
            if src_count == 0 {
                return None;
            }
            let src = TabPath::new(
                src_path.surface,
                src_path.node,
                TabIndex(usize::from(src_tab) % src_count),
            );
            // The insertion index is deliberately allowed to reach `dst_count` (the append
            // position) and to be computed against the *pre-removal* count — that is exactly
            // the out-of-range case `move_tab` has to clamp.
            let insert = match insert % 6 {
                0 => TabInsert::Append,
                1 => TabInsert::Insert(TabIndex(dst_count)),
                2 => TabInsert::Insert(TabIndex(dst_count.saturating_sub(1))),
                3 => TabInsert::Split(Split::Left),
                4 => TabInsert::Split(Split::Below),
                _ => TabInsert::Insert(TabIndex(0)),
            };
            // Whether the move changed anything is up to the generated operands (a generated
            // move can land on the slot the tab is already in); the invariants checked after
            // every op do not depend on it.
            let _ = state.move_tab(src, (dst_path, insert));
            vec![src_path, dst_path]
        }

        Op::SetActive { leaf, tab } => {
            let path = pick(leaf);
            let count = state[path].tabs_count();
            if count == 0 {
                return None;
            }
            let _ = state.set_active_tab(TabPath::new(
                path.surface,
                path.node,
                TabIndex(usize::from(tab) % count),
            ));
            // Which tab is open does not change *which tabs are there*.
            vec![]
        }

        Op::Focus { leaf } => {
            state.set_focused_node_and_surface(pick(leaf));
            vec![]
        }

        Op::ToggleCollapsed { leaf } => {
            let path = pick(leaf);
            let collapsed = state[path].is_collapsed();
            state[path.surface].set_leaf_collapsed(path.node, !collapsed);
            // Collapsing hides a leaf's tabs; it does not touch which tabs are where.
            vec![]
        }

        Op::ToggleStowed { split } => {
            // Splits, unlike leaves, are not in `live` — a dock can be all leaf and have none,
            // in which case the step is skipped rather than aimed somewhere else.
            let splits: Vec<NodePath> = state
                .iter_all_nodes()
                .filter(|(_, node)| node.is_parent())
                .map(|(path, _)| path)
                .collect();
            if splits.is_empty() {
                return None;
            }
            let path = splits[usize::from(split) % splits.len()];
            let stowed = state[path].is_stowed();
            state[path.surface].set_split_stowed(path.node, !stowed);
            // Putting a subtree away hides tabs; it does not move any.
            vec![]
        }

        Op::PushToFocused => {
            state.push_to_focused_leaf(fresh_tab());
            push_landing(state)
        }

        Op::Detach { leaf, tab } => {
            let path = pick(leaf);
            let count = state[path].tabs_count();
            if count == 0 {
                return None;
            }
            let src = TabPath::new(path.surface, path.node, TabIndex(usize::from(tab) % count));
            // Where the window opens is state, but not state these oracles judge; a fixed rect
            // keeps the generated bytes spent on structure instead of on geometry.
            let window = state.detach_tab(
                src,
                Rect::from_min_size(Point::new(64.0, 48.0), Size::new(320.0, 240.0)),
            );
            let mut touched = vec![path];
            touched.extend(leaves_of(state, window));
            touched
        }

        Op::AddWindow { tabs } => {
            let tabs: Vec<u32> = (0..fresh_tab_count(tabs)).map(|_| fresh_tab()).collect();
            let surface = state.add_window(tabs);
            leaves_of(state, surface)
        }

        Op::RemoveWindow { window } => {
            let candidates = open_windows(state);
            if candidates.is_empty() {
                return None;
            }
            let closed = candidates[usize::from(window) % candidates.len()];
            let touched = leaves_of(state, SurfaceIndex::Window(closed));
            state.remove_window(closed);
            touched
        }

        Op::RetainOdd => {
            state.retain_tabs(|tab| *tab % 2 == 1);
            // A sweep is about every leaf there is, so nothing is off limits for it.
            leaves(state)
        }

        Op::Remap => {
            let before = layout_by_surface(state);
            let mapped = state.map_tabs(|tab| *tab);
            assert_eq!(
                layout_by_surface(&mapped),
                before,
                "an identity map must rename nothing"
            );
            *state = mapped;
            leaves(state)
        }

        Op::FilterOddCopying => {
            *state = state.filter_tabs(|tab| *tab % 2 == 1);
            leaves(state)
        }
    };

    applied(touched)
}

/// The leaf a `push_to_focused_leaf` just landed in — or, if that cannot be named without
/// copying the implementation, every leaf there is.
///
/// When a leaf is focused, the push went there and the answer is exact. When none is,
/// `push_to_focused_leaf` falls back to "the first available leaf, or a new one", and naming
/// that here would mean restating the rule inside the very method under test — the shape this
/// module exists to stop. So the fallback is deliberately *conservative*: it reports every
/// leaf, which excludes the step from the identity property rather than asserting a guess.
fn push_landing<Tab>(state: &DockState<Tab>) -> Vec<NodePath> {
    match state.focused_leaf() {
        Some(path) => vec![path],
        None => leaves(state),
    }
}

/// Leaves of one surface, or nothing if that surface holds no tree.
fn leaves_of<Tab>(state: &DockState<Tab>, surface: SurfaceIndex) -> Vec<NodePath> {
    state
        .iter_leaves()
        .filter(|(path, _)| path.surface == surface)
        .map(|(path, _)| path)
        .collect()
}

/// The layout of every surface that holds a tab, keyed by its index.
///
/// This is the oracle of a *copying sweep*, and deliberately **not**
/// [`crate::core::shape::dock_shape`], which answers a different question (did the layout
/// survive being written to a file and read back). Two differences, both load-bearing:
///
/// * surfaces without tabs are skipped here. That is not a separate rule any more: a surface
///   with no tabs is a surface with no root (an empty dock has exactly one shape — see
///   [`Tree::new`](crate::core::tree::Tree::new)), so asking for the root is the whole of it.
///   It used to be one, because a sweep could leave an empty root leaf behind;
/// * trailing holes in the window vector may legally be popped, which shifts nothing that has
///   tabs in it.
///
/// The per-subtree string itself is [`crate::core::shape::subtree_shape`], shared with the DST
/// trace: the two oracles say different things, but they say them in the same language, and a
/// language re-derived per call site drifts.
pub fn layout_by_surface(state: &DockState<u32>) -> Vec<(SurfaceIndex, String)> {
    state
        .iter_surfaces_indexed()
        .filter_map(|(index, surface)| {
            let tree = surface.node_tree()?;
            // A surface holding no tabs has no root, and drops out here — see the doc comment
            // for why that is not a blind spot but the point.
            let root = tree.root()?;
            Some((index, crate::core::shape::subtree_shape(tree, root)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The conservation oracle has to *bite*, in both directions.
    ///
    /// A rule stated as data is easy to state wrongly (say, by treating every op as
    /// `MayShrink`), and a wrong rule is invisible: every harness using it just goes green.
    #[test]
    fn the_tab_count_rule_says_what_each_op_may_do() {
        let split = Op::Split {
            leaf: 0,
            split: 0,
            tabs: 0,
        };
        assert_eq!(tab_count_rule(split), TabCountRule::Adds(1));
        assert!(check_tab_count(split, 3, 4).is_ok());
        assert!(
            check_tab_count(split, 3, 3).is_err(),
            "a split that added nothing must be reported"
        );
        assert!(
            check_tab_count(split, 3, 5).is_err(),
            "a split that added too much must be reported"
        );

        let mv = Op::MoveTab {
            src_leaf: 0,
            src_tab: 0,
            dst_leaf: 0,
            insert: 0,
        };
        assert_eq!(tab_count_rule(mv), TabCountRule::Unchanged);
        assert!(check_tab_count(mv, 3, 3).is_ok());
        assert!(
            check_tab_count(mv, 3, 2).is_err(),
            "a move that lost a tab must be reported"
        );

        assert_eq!(tab_count_rule(Op::RetainOdd), TabCountRule::MayShrink);
        assert!(check_tab_count(Op::RetainOdd, 4, 2).is_ok());
        assert!(
            check_tab_count(Op::RetainOdd, 4, 5).is_err(),
            "a sweep that invented a tab must be reported"
        );
    }

    /// Every operation must be applicable to the dock it is handed, or say it is not.
    ///
    /// Cheap smoke test with a real purpose: `apply` picks its targets modulo the live leaves,
    /// so an op whose arithmetic is off panics rather than misbehaves, and a panic inside a
    /// generator-driven harness reads as "the fuzzer found something" rather than "the harness
    /// is broken".
    #[test]
    fn every_op_applies_to_a_two_leaf_dock_or_declines() {
        let ops = [
            Op::Split {
                leaf: 1,
                split: 2,
                tabs: 2,
            },
            Op::RemoveTab { leaf: 0, tab: 0 },
            Op::MoveTab {
                src_leaf: 0,
                src_tab: 0,
                dst_leaf: 1,
                insert: 3,
            },
            Op::SetActive { leaf: 0, tab: 1 },
            Op::Focus { leaf: 1 },
            Op::ToggleCollapsed { leaf: 0 },
            Op::PushToFocused,
            Op::Detach { leaf: 0, tab: 0 },
            Op::AddWindow { tabs: 1 },
            Op::RemoveWindow { window: 0 },
            Op::RetainOdd,
            Op::Remap,
            Op::FilterOddCopying,
            Op::RemoveLeaf { leaf: 0 },
        ];

        for op in ops {
            let mut state = DockState::new(vec![0u32, 1, 2]);
            let root = state.main_surface().root().unwrap();
            state.split(
                NodePath {
                    surface: SurfaceIndex::Main,
                    node: root,
                },
                Split::Right,
                0.5,
                Node::leaf_with(vec![3u32, 4]),
            );
            let mut next_tab = 5u32;

            let before = total_tabs(&state);
            let Some(applied) = apply(&mut state, op, &mut next_tab) else {
                continue;
            };
            assert_eq!(
                state.validate(),
                Ok(()),
                "{op:?} left the dock invalid on a two-leaf scene"
            );
            check_tab_count(op, before, total_tabs(&state)).unwrap();
            for path in &applied.touched {
                assert!(
                    state.is_surface_valid(path.surface) || state[path.surface].is_empty(),
                    "{op:?} reported a touched leaf on a surface that is not there: {path:?}"
                );
            }
        }
    }
}
