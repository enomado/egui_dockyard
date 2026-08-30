//! Textual dump of a dock's shape — the thing worth comparing before and after a round-trip.
//!
//! # Why it lives here and not at each caller
//!
//! The same dump was needed three times: by the layout-corpus gates in the application, by the
//! format gate that runs without the application (`dock_layout_gate`) and — in its own variant
//! — by the DST simulator. The first two assert THE SAME property (did the shape survive a
//! write and a read), and when the rule is re-derived at every call site the copies drift
//! apart silently: that is exactly how the two surface sweeps drifted on P3.
//!
//! The DST trace is deliberately NOT folded in here: it answers a different question —
//! reproducibility of a run from its seed — so it carries neither split fractions nor
//! collapsed counters, but does carry the active tab of every leaf. That is not "one more
//! copy", it is a different claim.
//!
//! # What the dump contains, and why
//!
//! * Surfaces are numbered by their position in the STORED shape (main = 0, window N → N+1):
//!   the dump is compared against the dump of the same dock after a round-trip, while in
//!   memory main lives in its own field and windows count from zero (P6). Without that
//!   translation the dump would be catching its own addressing rather than the format.
//! * Nodes are numbered by breadth-first traversal, NOT by identity: a dock before and after
//!   saving has deliberately different identities — they do not survive the file and must not.
//! * Split fractions and collapsed counters are part of the dump: a writer that loses
//!   `fraction` has to be visible (on P2 that mutation failed 8 of 13 layouts).

use std::fmt::{Display, Write};

use crate::core::DockState;
use crate::core::surface_index::SurfaceIndex;
use crate::core::tree::Tree;
use crate::core::tree::node::Node;
use crate::core::tree::node_id::NodeId;

/// The shape of one subtree on a single line: split orientations, nesting, and the tabs of
/// every leaf in order. A leaf is `[a,b,]`, a split is `V(left|right)` / `H(...)`.
///
/// Node identities are deliberately NOT part of the dump: the copying sweep builds a fresh
/// arena and hands out fresh ids, so slot order is an implementation detail, whereas "which
/// split holds what, and in which order" is exactly what the user sees.
///
/// # Why it lives here and not at each caller
///
/// This very string is needed by two different claims — the oracle of the copying sweep
/// (`core::testkit::layout_by_surface`) and the trace of the DST simulator — and until
/// 2026-08-08 it lived as two copies which had already drifted apart (`is_vertical()` versus a
/// hand-written `matches!`). Different claims are entitled to their own dumps (see the module
/// header on `dock_shape`), but the PRIMITIVE they are assembled from must not drift: two
/// copies of one string are the same shape this track has already been bitten by twice.
///
/// The dump is meant for COMPARISON, not for parsing: the format is free to change as long as
/// it changes identically for both sides of the comparison.
pub fn subtree_shape<Tab: Display>(tree: &Tree<Tab>, id: NodeId) -> String {
    let mut out = String::new();
    write_subtree_shape(tree, id, &mut out);
    out
}

fn write_subtree_shape<Tab: Display>(tree: &Tree<Tab>, id: NodeId, out: &mut String) {
    match &tree[id] {
        Node::Leaf(leaf) => {
            out.push('[');
            for tab in leaf.iter_tabs() {
                write!(out, "{tab},").unwrap();
            }
            out.push(']');
        }
        node => {
            out.push(if node.is_vertical() { 'V' } else { 'H' });
            out.push('(');
            // Written as a loop over however many children there are, not as a pair: the
            // language then already says `H(a|b|c)` on the day a row can hold three, and for
            // two children it is character for character what it always wrote — which is what
            // `the_subtree_shape_writes_splits_nesting_and_tabs_in_order` is here to hold.
            for (position, child) in tree.children(id).unwrap().iter().enumerate() {
                if position > 0 {
                    out.push('|');
                }
                write_subtree_shape(tree, *child, out);
            }
            out.push(')');
        }
    }
}

/// Position of a surface in the stored shape: main is always 0, window `n` sits at `n + 1`.
fn stored_position(index: SurfaceIndex) -> usize {
    match index {
        SurfaceIndex::Main => 0,
        SurfaceIndex::Window(window) => window.0 + 1,
    }
}

/// The shape of a dock as text: surfaces, nodes, split fractions, tab contents and order.
///
/// `tab_label` names a tab however the caller finds convenient (`Debug`, a title, a variant
/// name) — the shape itself does not depend on that, but a difference in tab contents has to
/// be readable.
///
/// The dump is meant for COMPARISON (before/after saving), not for parsing: its format may
/// change as long as it changes identically for both sides of the comparison.
pub fn dock_shape<Tab>(state: &DockState<Tab>, tab_label: impl Fn(&Tab) -> String) -> String {
    let mut out = String::new();
    for (surface_index, surface) in state.iter_surfaces_indexed() {
        let Some(tree) = surface.node_tree() else {
            writeln!(out, "surface {}: empty", stored_position(surface_index)).unwrap();
            continue;
        };
        let order = tree.breadth_first();
        let position = |id: NodeId| order.iter().position(|other| *other == id);
        writeln!(
            out,
            "surface {}: {} nodes, focus {:?}",
            stored_position(surface_index),
            order.len(),
            tree.focused_leaf().and_then(position)
        )
        .unwrap();
        for (index, id) in order.iter().enumerate() {
            match &tree[*id] {
                Node::Leaf(leaf) => {
                    let tabs: Vec<String> = leaf.iter_tabs().map(&tab_label).collect();
                    writeln!(
                        out,
                        "  {index}: leaf active={:?} collapsed={} tabs={tabs:?}",
                        leaf.active_index().map(|i| i.0),
                        leaf.collapsed
                    )
                    .unwrap();
                }
                Node::Row(row) => {
                    writeln!(
                        out,
                        "  {index}: {} fraction={} collapsed={} children={:?}",
                        if row.is_vertical() {
                            "vertical"
                        } else {
                            "horizontal"
                        },
                        row.fraction(),
                        row.fully_collapsed,
                        // A `Vec` where this used to hand `[Option<usize>; 2]` to `{:?}`: both
                        // print `[Some(1), Some(2)]`, so the dump is unchanged while the count
                        // stops being fixed.
                        row.children()
                            .iter()
                            .map(|child| position(*child))
                            .collect::<Vec<_>>(),
                    )
                    .unwrap();
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tree::Split;
    use crate::core::tree::node::Node;

    /// The language of `subtree_shape` is pinned to the letter: it now has TWO callers with
    /// different claims (the oracle of the copying sweep and the DST trace), and both compare
    /// a dump against a dump — that is, a format that drifts would be survived by both in
    /// silence. It is checked literally here because there is nowhere else: at both callers
    /// the two sides of the comparison drift together.
    #[test]
    fn the_subtree_shape_writes_splits_nesting_and_tabs_in_order() {
        let mut state = DockState::new(vec!["a".to_string()]);
        let root = state.main_surface().root().unwrap();
        let [_left, right] =
            state
                .main_surface_mut()
                .split(root, Split::Below, 0.5, Node::leaf("b".to_string()));
        state
            .main_surface_mut()
            .leaf_mut(right)
            .unwrap()
            .append_tab("c".to_string());
        let root = state.main_surface().root().unwrap();

        assert_eq!(
            subtree_shape(state.main_surface(), root),
            "V([a,]|[b,c,])",
            "the dump format drifted — at the callers both sides of the comparison drift with \
             it and notice nothing"
        );
    }

    /// The language of `dock_shape` is pinned to the letter, for the reason given above
    /// `the_subtree_shape_writes_splits_nesting_and_tabs_in_order` and with one more caller
    /// behind it: the n-ary plan spends three stages proving parity by comparing this dump
    /// before a refactor with the same dump after it, and both sides of *that* comparison drift
    /// together too. A writer that consistently mangled a field would leave every one of those
    /// gates green while saying something untrue about the tree — which is exactly what a
    /// mutation reversing the `children=` list did until this test existed.
    #[test]
    fn the_dock_shape_writes_a_line_per_node_naming_children_by_position() {
        let mut state = DockState::new(vec!["a".to_string()]);
        let root = state.main_surface().root().unwrap();
        state
            .main_surface_mut()
            .split(root, Split::Right, 0.25, Node::leaf("b".to_string()));

        assert_eq!(
            dock_shape(&state, |tab| tab.clone()),
            "surface 0: 3 nodes, focus Some(2)\n  \
             0: horizontal fraction=0.25 collapsed=false children=[Some(1), Some(2)]\n  \
             1: leaf active=Some(0) collapsed=false tabs=[\"a\"]\n  \
             2: leaf active=Some(0) collapsed=false tabs=[\"b\"]\n",
            "the dump format drifted — the parity gates that compare it against itself notice \
             nothing"
        );
    }

    /// The dump has to TELL APART docks that differ only in what it is meant to judge.
    ///
    /// A tooth against "a difference for free": were the dump to print the tree shape alone,
    /// two layouts with different split fractions would read identically, and the round-trip
    /// gate would silently stop catching a lost `fraction`.
    #[test]
    fn the_shape_notices_a_fraction_a_focus_and_a_tab_order() {
        let mut state = DockState::new(vec!["a".to_string()]);
        let root = state.main_surface().root().unwrap();
        let [left, _right] =
            state
                .main_surface_mut()
                .split(root, Split::Right, 0.5, Node::leaf("b".to_string()));
        let base = dock_shape(&state, |tab| tab.clone());

        // Split fraction.
        let mut moved = state.clone();
        let split_id = moved.main_surface().root().unwrap();
        match &mut moved.main_surface_mut()[split_id] {
            Node::Row(row) => row.set_fraction(0.25),
            Node::Leaf(_) => unreachable!("the scene's root is a row by construction"),
        }
        assert_ne!(
            base,
            dock_shape(&moved, |tab| tab.clone()),
            "the dump missed a changed split fraction"
        );

        // Focus.
        let mut focused = state.clone();
        focused.main_surface_mut().set_focused_node(left);
        assert_ne!(
            base,
            dock_shape(&focused, |tab| tab.clone()),
            "the dump missed a moved focus"
        );

        // Tab contents.
        let mut extra = state.clone();
        extra
            .main_surface_mut()
            .leaf_mut(left)
            .unwrap()
            .append_tab("c".to_string());
        assert_ne!(
            base,
            dock_shape(&extra, |tab| tab.clone()),
            "the dump missed an added tab"
        );
    }
}
