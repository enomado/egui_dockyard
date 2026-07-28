#![no_main]
//! Coverage-guided sibling of the property tests in `src/proptests.rs`.
//!
//! The property tests draw operation sequences blind; libFuzzer draws them with feedback from
//! the code it just executed, so it gets to *aim* — at the branch that only runs when a leaf
//! is emptied by a move, at the surface bookkeeping that only runs when a window loses its
//! last tab. Two things follow from that difference, and they are the reason this target
//! exists rather than a bigger `proptest!` block:
//!
//! * **windows are in scope here.** The property tests stay on the main surface; detaching a
//!   tab into a window, moving tabs back out of it and closing surfaces is exactly the
//!   bookkeeping (`remove_surface`, empty-surface cleanup) that no test drives today.
//! * **sequences may be long.** 64 operations against a tree that keeps growing is not a
//!   size a property test can afford per case.
//!
//! Oracles, in the order they bite:
//!
//! 1. [`DockState::validate`] after *every* operation, so the report names the operation that
//!    broke the tree rather than the end of the sequence;
//! 2. tab conservation — an operation that is not supposed to destroy anything must not
//!    change the total tab count. Without this half, an implementation that "repaired" a
//!    broken move by dropping the tab would keep every structural invariant and pass.
//!
//! Identity (`ids_keep_naming_the_same_node`) is deliberately left to the property tests: it
//! needs a before/after snapshot per step, which is too slow to run at fuzzing rates.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;

use egui_dock::core::geom::{Point, Rect, Size};
use egui_dock::{
    DockState, Node, NodePath, Split, SurfaceIndex, TabIndex, TabInsert, TabPath, WindowIndex,
};

/// One operation applied to the dock state.
///
/// Nodes are addressed as "the k-th live leaf", not by id: ids are handed out by the arena and
/// cannot be invented by a fuzzer, so the operation picks its target at apply time and `k` is
/// taken modulo the number of live leaves. The same goes for surfaces.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum Op {
    Split { leaf: u8, split: u8, tabs: u8 },
    RemoveLeaf { leaf: u8 },
    RemoveTab { leaf: u8, tab: u8 },
    MoveTab { src_leaf: u8, src_tab: u8, dst_leaf: u8, insert: u8 },
    SetActive { leaf: u8, tab: u8 },
    Focus { leaf: u8 },
    PushToFocused,
    /// Pull a tab out into its own window — the only op that creates a surface by hand.
    Detach { leaf: u8, tab: u8 },
    AddWindow { tabs: u8 },
    /// Close a whole window. The main surface is never a candidate (`remove_surface` asserts).
    RemoveSurface { surface: u8 },
    /// Drop every tab whose value is odd — a destructive sweep that touches every surface at
    /// once, which no other op does.
    RetainOdd,
    /// Rebuild the whole dock through the copying sweep (`map_tabs`) with an identity
    /// function. Structurally this is the widest operation there is — every surface and every
    /// node is rebuilt — and it must still be invisible: same tabs, in the same surfaces,
    /// under the same indices.
    Remap,
    /// The copying sibling of `RetainOdd`: same sweep, but through `filter_tabs`, which builds
    /// a new dock instead of editing this one. The two paths keep the surface vector in
    /// different code, so a fix to one says nothing about the other.
    FilterOddCopying,
}

/// Whether an operation is allowed to reduce the total number of tabs.
fn is_destructive(op: Op) -> bool {
    matches!(
        op,
        Op::RemoveLeaf { .. }
            | Op::RemoveTab { .. }
            | Op::RemoveSurface { .. }
            | Op::RetainOdd
            | Op::FilterOddCopying
    )
}

/// The layout of one subtree, written down: split orientations, nesting, and the tabs of each
/// leaf in order.
///
/// Node *ids* are deliberately not part of it. A copying sweep builds a fresh arena and hands
/// out fresh ids, so slot order is an implementation detail — whereas which split holds what,
/// and in which order, is exactly what the user sees.
fn shape_of(tree: &egui_dock::Tree<u32>, id: egui_dock::NodeId, out: &mut String) {
    match &tree[id] {
        Node::Leaf(leaf) => {
            out.push('[');
            for tab in leaf.iter_tabs() {
                out.push_str(&format!("{tab},"));
            }
            out.push(']');
        }
        node => {
            out.push(if matches!(node, Node::Vertical(_)) { 'V' } else { 'H' });
            let [first, second] = tree.children(id).unwrap();
            out.push('(');
            shape_of(tree, first, out);
            out.push('|');
            shape_of(tree, second, out);
            out.push(')');
        }
    }
}

/// The layout of every surface that holds a tab, keyed by its index.
///
/// Surfaces without tabs are skipped, for two independent reasons:
///
/// * trailing holes may legally be popped, and that shifts nothing that has tabs in it;
/// * an empty dock has more than one legal shape — a tree with an empty root leaf (what
///   `ensure_tree` rebuilds) and a tree with no root at all (what a copying sweep leaves).
///   `filter_none_then_push` pins that the difference is invisible to the next operation, so
///   it is not something this oracle should be reporting.
fn layout_by_surface(state: &DockState<u32>) -> Vec<(SurfaceIndex, String)> {
    state
        .iter_surfaces_indexed()
        .filter_map(|(index, surface)| {
            let tree = surface.node_tree()?;
            let root = tree.root()?;
            if surface.iter_all_tabs().next().is_none() {
                return None;
            }
            let mut shape = String::new();
            shape_of(tree, root, &mut shape);
            Some((index, shape))
        })
        .collect()
}

fn split_from(index: u8) -> Split {
    match index % 4 {
        0 => Split::Left,
        1 => Split::Right,
        2 => Split::Above,
        _ => Split::Below,
    }
}

/// Every leaf of every live surface, in a stable order.
fn leaves(state: &DockState<u32>) -> Vec<NodePath> {
    state.iter_leaves().map(|(path, _)| path).collect()
}

/// Total number of tabs held anywhere in the dock state.
fn total_tabs(state: &DockState<u32>) -> usize {
    state.iter_all_tabs().count()
}

/// Surfaces that exist and are not the main one — the ones that may legally be removed.
fn removable_surfaces(state: &DockState<u32>) -> Vec<WindowIndex> {
    state
        .iter_surfaces_indexed()
        // The main surface filters itself out: it has no `WindowIndex` to offer, so it cannot
        // even be a candidate for closing.
        .filter_map(|(index, surface)| (!surface.is_empty()).then(|| index.as_window()).flatten())
        .collect()
}

/// Applies one operation, or reports that it was not applicable at all.
///
/// "Not applicable" is a real state here, not a failure: a sequence can empty the dock out, and
/// an op that needs a tab to work on then has nothing to do. Such a step is skipped rather than
/// counted, so the conservation oracle is not asked about an operation that never ran.
fn apply(state: &mut DockState<u32>, op: Op, next_tab: &mut u32) -> bool {
    let mut fresh_tab = || {
        let tab = *next_tab;
        *next_tab += 1;
        tab
    };

    let live = leaves(state);
    if live.is_empty() {
        // Only these two can rebuild a dock from nothing.
        return match op {
            Op::PushToFocused => {
                state.push_to_focused_leaf(fresh_tab());
                true
            }
            Op::AddWindow { tabs } => {
                let tabs: Vec<u32> = (0..=(tabs % 3)).map(|_| fresh_tab()).collect();
                state.add_window(tabs);
                true
            }
            _ => false,
        };
    }
    let pick = |k: u8| live[usize::from(k) % live.len()];

    match op {
        Op::Split { leaf, split, tabs } => {
            let path = pick(leaf);
            let tabs: Vec<u32> = (0..=(tabs % 3)).map(|_| fresh_tab()).collect();
            state.split(path, split_from(split), 0.5, Node::leaf_with(tabs));
        }

        Op::RemoveLeaf { leaf } => state.remove_leaf(pick(leaf)),

        Op::RemoveTab { leaf, tab } => {
            let path = pick(leaf);
            let count = state[path].tabs_count();
            if count == 0 {
                return false;
            }
            state.remove_tab(TabPath::new(
                path.surface,
                path.node,
                TabIndex(usize::from(tab) % count),
            ));
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
                return false;
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
        }

        Op::SetActive { leaf, tab } => {
            let path = pick(leaf);
            let count = state[path].tabs_count();
            if count == 0 {
                return false;
            }
            let _ = state.set_active_tab(TabPath::new(
                path.surface,
                path.node,
                TabIndex(usize::from(tab) % count),
            ));
        }

        Op::Focus { leaf } => state.set_focused_node_and_surface(pick(leaf)),

        Op::PushToFocused => state.push_to_focused_leaf(fresh_tab()),

        Op::Detach { leaf, tab } => {
            let path = pick(leaf);
            let count = state[path].tabs_count();
            if count == 0 {
                return false;
            }
            let src = TabPath::new(
                path.surface,
                path.node,
                TabIndex(usize::from(tab) % count),
            );
            // Where the window opens is state, but not state this oracle judges; a fixed rect
            // keeps the input bytes spent on structure instead of on geometry.
            state.detach_tab(
                src,
                Rect::from_min_size(Point::new(64.0, 48.0), Size::new(320.0, 240.0)),
            );
        }

        Op::AddWindow { tabs } => {
            let tabs: Vec<u32> = (0..=(tabs % 3)).map(|_| fresh_tab()).collect();
            state.add_window(tabs);
        }

        Op::RemoveSurface { surface } => {
            let candidates = removable_surfaces(state);
            if candidates.is_empty() {
                return false;
            }
            state.remove_window(candidates[usize::from(surface) % candidates.len()]);
        }

        Op::RetainOdd => state.retain_tabs(|tab| *tab % 2 == 1),

        Op::Remap => {
            let before = layout_by_surface(state);
            let mapped = state.map_tabs(|tab| *tab);
            assert_eq!(
                layout_by_surface(&mapped),
                before,
                "an identity map must rename nothing"
            );
            *state = mapped;
        }

        Op::FilterOddCopying => *state = state.filter_tabs(|tab| *tab % 2 == 1),
    }

    true
}

fuzz_target!(|ops: Vec<Op>| {
    let mut state = DockState::new(vec![0u32, 1, 2]);
    let mut next_tab = 3u32;

    // A run that starts from a broken state would blame the first operation for it.
    assert_eq!(
        state.validate(),
        Ok(()),
        "the initial dock state must be well-formed"
    );

    // Long sequences are the point, but not unbounded ones: past this the run is spending its
    // time growing a tree rather than exploring branches.
    for (step, op) in ops.into_iter().take(64).enumerate() {
        let before = total_tabs(&state);
        if !apply(&mut state, op, &mut next_tab) {
            continue;
        }
        let after = total_tabs(&state);

        if let Err(violations) = state.validate() {
            panic!("step {step} ({op:?}) left the dock state invalid: {violations:?}");
        }

        match op {
            Op::Split { tabs, .. } | Op::AddWindow { tabs } => {
                let added = usize::from(tabs % 3) + 1;
                assert_eq!(
                    after,
                    before + added,
                    "step {step} ({op:?}) must add exactly the tabs it was given"
                );
            }
            Op::PushToFocused => assert_eq!(
                after,
                before + 1,
                "step {step} ({op:?}) must add exactly one tab"
            ),
            op if !is_destructive(op) => assert_eq!(
                after, before,
                "step {step} ({op:?}) must not change the tab count"
            ),
            _ => assert!(
                after <= before,
                "step {step} ({op:?}) is destructive but invented tabs"
            ),
        }
    }
});
