//! Reading and writing the persisted shape of a [`Tree`].
//!
//! # Why this is hand-written
//!
//! The in-memory tree is an arena: nodes are addressed by [`NodeId`], which is a slot plus
//! a generation and means nothing outside the process that handed it out. What a saved
//! layout has to describe is the *shape* — who contains whom, in what order, with which
//! split fractions — so the wire format is a plain recursive tree, and loading rebuilds a
//! fresh arena from it. Positions (a tab's place in its leaf, a path of
//! [`Side`]s to the focused node) are the right currency here precisely because there is no
//! identity to carry across a save.
//!
//! # Two readers, one writer
//!
//! Layouts written before the arena hold the old representation: an implicit binary heap in
//! a `Vec`, children of *n* at *2n + 1* / *2n + 2*, holes spelled `Empty`, focus as an
//! index into that `Vec`. Those files exist on users' disks, so the reader accepts both
//! forms and the writer only ever emits the new one. Telling them apart needs no
//! guessing and no `deserialize_any`: the two shapes use disjoint field names (`root` vs
//! `nodes`), everything is optional, and whichever one is populated decides.
//!
//! The new form is also markedly smaller: a heap has to spell out every hole, so a deeply
//! unbalanced layout used to serialize `2^depth` slots — the corpus has a real file with
//! 218 `Empty` entries.

use std::collections::HashMap;

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::{LeafNode, Node, NodeId, Side, SplitNode, TabIndex, Tree};

use super::arena::NodeEntry;

// ----------------------------------------------------------------------------
// What we write

#[derive(Serialize)]
struct TreeOut<'a, Tab> {
    root: Option<NodeOut<'a, Tab>>,
    /// Route from the root to the focused leaf, or `None` if nothing is focused.
    focused: Option<Vec<Side>>,
    collapsed: bool,
    collapsed_leaf_count: i32,
}

#[derive(Serialize)]
enum NodeOut<'a, Tab> {
    Leaf {
        tabs: Vec<&'a Tab>,
        active: TabIndex,
        prev_active: Option<TabIndex>,
        scroll: f32,
        collapsed: bool,
    },
    Vertical {
        fraction: f32,
        fully_collapsed: bool,
        collapsed_leaf_count: i32,
        children: [Box<NodeOut<'a, Tab>>; 2],
    },
    Horizontal {
        fraction: f32,
        fully_collapsed: bool,
        collapsed_leaf_count: i32,
        children: [Box<NodeOut<'a, Tab>>; 2],
    },
}

fn node_out<Tab>(tree: &Tree<Tab>, id: NodeId) -> NodeOut<'_, Tab> {
    match &tree[id] {
        Node::Leaf(leaf) => NodeOut::Leaf {
            tabs: leaf.iter_tabs().collect(),
            // An empty leaf has no active tab at all; the old format could not say that,
            // so it gets the position it always had there.
            active: leaf.active_index().unwrap_or(TabIndex(0)),
            prev_active: leaf.prev_active_index(),
            scroll: leaf.scroll,
            collapsed: leaf.collapsed,
        },
        node @ (Node::Vertical(split) | Node::Horizontal(split)) => {
            let [left, right] = split.children();
            let children = [
                Box::new(node_out(tree, left)),
                Box::new(node_out(tree, right)),
            ];
            let fraction = split.fraction;
            let fully_collapsed = split.fully_collapsed;
            let collapsed_leaf_count = split.collapsed_leaf_count;
            if node.is_vertical() {
                NodeOut::Vertical {
                    fraction,
                    fully_collapsed,
                    collapsed_leaf_count,
                    children,
                }
            } else {
                NodeOut::Horizontal {
                    fraction,
                    fully_collapsed,
                    collapsed_leaf_count,
                    children,
                }
            }
        }
    }
}

/// The route from the root down to `node`, as the sequence of turns to take.
fn path_to<Tab>(tree: &Tree<Tab>, node: NodeId) -> Option<Vec<Side>> {
    let mut path = Vec::new();
    let mut current = node;
    while let Some(parent) = tree.parent(current) {
        let side = tree[parent].get_split()?.side_of(current)?;
        path.push(side);
        current = parent;
    }
    // A path is only meaningful if walking up actually arrived at the root.
    (Some(current) == tree.root()).then(|| {
        path.reverse();
        path
    })
}

impl<Tab: Serialize> Serialize for Tree<Tab> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        TreeOut {
            root: self.root().map(|root| node_out(self, root)),
            focused: self.focused_leaf().and_then(|node| path_to(self, node)),
            collapsed: self.is_collapsed(),
            collapsed_leaf_count: self.collapsed_leaf_count(),
        }
        .serialize(serializer)
    }
}

// ----------------------------------------------------------------------------
// What we read

#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
struct TreeIn<Tab> {
    // The current form.
    #[serde(default = "none")]
    root: Option<NodeIn<Tab>>,
    #[serde(default = "none")]
    focused: Option<Vec<Side>>,

    // The pre-arena form: an implicit binary heap plus an index into it.
    #[serde(default = "Vec::new")]
    nodes: Vec<LegacyNode<Tab>>,
    #[serde(default = "none")]
    focused_node: Option<LegacyNodeIndex>,
    // `collapsed` / `collapsed_leaf_count` are in every file both forms ever wrote, and are
    // deliberately absent here: they are derived from the leaves, and reading recomputes them.
    // See `Deserialize for Tree`.
}

/// `#[serde(default)]` on a generic field would demand `Tab: Default`; this does not.
fn none<T>() -> Option<T> {
    None
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
enum NodeIn<Tab> {
    Leaf {
        tabs: Vec<Tab>,
        active: TabIndex,
        #[serde(default = "none")]
        prev_active: Option<TabIndex>,
        #[serde(default)]
        scroll: f32,
        #[serde(default)]
        collapsed: bool,
    },
    // A split stores `fully_collapsed` / `collapsed_leaf_count` on disk as well; they are
    // unknown fields here on purpose, for the same reason the tree-level pair is — the
    // numbers follow from `Leaf::collapsed`, which is read.
    Vertical {
        fraction: f32,
        children: [Box<NodeIn<Tab>>; 2],
    },
    Horizontal {
        fraction: f32,
        children: [Box<NodeIn<Tab>>; 2],
    },
}

/// A node of the pre-arena heap. `rect` / `viewport`, which older files also carry, are
/// unknown fields here and are ignored — geometry stopped being state before the arena did.
#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
enum LegacyNode<Tab> {
    Empty,
    Leaf(LegacyLeaf<Tab>),
    Vertical(LegacySplit),
    Horizontal(LegacySplit),
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
struct LegacyLeaf<Tab> {
    tabs: Vec<Tab>,
    active: TabIndex,
    #[serde(default = "none")]
    prev_active: Option<TabIndex>,
    #[serde(default)]
    scroll: f32,
    #[serde(default)]
    collapsed: bool,
}

/// Same as the current form: the collapsing numbers the file carries are ignored and
/// recomputed, only `fraction` is genuine state.
#[derive(Deserialize)]
struct LegacySplit {
    fraction: f32,
}

/// The old positional address: an index into the heap `Vec`.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
struct LegacyNodeIndex(usize);

impl<Tab> Tree<Tab> {
    /// Inserts a detached node and returns its id. The caller links it up.
    fn adopt(&mut self, node: Node<Tab>) -> NodeId {
        self.nodes.insert(NodeEntry { parent: None, node })
    }

    /// Links `children` under a freshly built split.
    fn adopt_split(&mut self, vertical: bool, children: [NodeId; 2], fraction: f32) -> NodeId {
        let split = SplitNode::new(children, fraction);
        let node = if vertical {
            Node::Vertical(split)
        } else {
            Node::Horizontal(split)
        };
        let id = self.adopt(node);
        for child in children {
            self.nodes.get_mut(child).unwrap().parent = Some(id);
        }
        id
    }

    /// Builds a subtree of the current form.
    ///
    /// Returns `None` when nothing of the subtree survives. That happens for a leaf holding no
    /// tabs: an empty leaf below the root is not a state the in-memory tree allows (removing
    /// the last tab collapses the leaf), so a file describing one is repaired on the way in
    /// rather than turned into a tree that fails its own invariants. A split left with a
    /// single surviving child is replaced by that child — the same repair the pre-arena
    /// reader already applies to a split that lost one.
    ///
    /// The root leaf is the documented exception (an empty dock is legitimate), and it is
    /// handled by the caller, which never routes it through here.
    fn build(&mut self, node: NodeIn<Tab>) -> Option<NodeId> {
        let vertical = matches!(node, NodeIn::Vertical { .. });
        match node {
            NodeIn::Leaf {
                tabs,
                active,
                prev_active,
                scroll,
                collapsed,
            } => {
                if tabs.is_empty() {
                    return None;
                }
                Some(self.build_leaf(tabs, active, prev_active, scroll, collapsed))
            }
            NodeIn::Vertical { fraction, children } | NodeIn::Horizontal { fraction, children } => {
                let [left, right] = children;
                let left = self.build(*left);
                let right = self.build(*right);
                match (left, right) {
                    (Some(left), Some(right)) => {
                        Some(self.adopt_split(vertical, [left, right], fraction))
                    }
                    (Some(only), None) | (None, Some(only)) => Some(only),
                    (None, None) => None,
                }
            }
        }
    }

    /// Adopts a leaf exactly as the file describes it, empty or not.
    fn build_leaf(
        &mut self,
        tabs: Vec<Tab>,
        active: TabIndex,
        prev_active: Option<TabIndex>,
        scroll: f32,
        collapsed: bool,
    ) -> NodeId {
        let leaf = LeafNode::from_persisted(tabs, active, prev_active, scroll, collapsed);
        self.adopt(Node::Leaf(leaf))
    }

    /// Builds a subtree of the pre-arena heap, starting at heap position `index`.
    ///
    /// Returns `None` for a hole (an `Empty` slot, or one past the end of the `Vec`).
    /// `where_it_landed` records heap position → id so that the old focus index, which is
    /// a heap position too, can be translated afterwards.
    fn build_legacy(
        &mut self,
        nodes: &mut Vec<Option<LegacyNode<Tab>>>,
        index: usize,
        where_it_landed: &mut HashMap<usize, NodeId>,
    ) -> Option<NodeId> {
        // `take` rather than index: a heap slot is consumed exactly once, and the tabs
        // inside it are moved rather than cloned (`Tab` need not be `Clone`).
        let node = nodes.get_mut(index).and_then(Option::take)?;
        let vertical_split = matches!(node, LegacyNode::Vertical(_));
        let id = match node {
            LegacyNode::Empty => return None,
            LegacyNode::Leaf(leaf) => {
                let LegacyLeaf {
                    tabs,
                    active,
                    prev_active,
                    scroll,
                    collapsed,
                } = leaf;
                // Same repair as in the current form: an empty leaf anywhere but at the root
                // is a state the tree does not allow, so it is dropped and its parent split
                // collapses onto the surviving sibling below.
                if tabs.is_empty() && index != 0 {
                    return None;
                }
                self.build_leaf(tabs, active, prev_active, scroll, collapsed)
            }
            LegacyNode::Vertical(split) | LegacyNode::Horizontal(split) => {
                let vertical = vertical_split;
                let left = self.build_legacy(nodes, index * 2 + 1, where_it_landed);
                let right = self.build_legacy(nodes, index * 2 + 2, where_it_landed);
                match (left, right) {
                    (Some(left), Some(right)) => {
                        self.adopt_split(vertical, [left, right], split.fraction)
                    }
                    // A split that lost a child in a stored file is repaired the way the
                    // in-memory tree repairs it: the surviving child takes its place.
                    (Some(only), None) | (None, Some(only)) => only,
                    (None, None) => return None,
                }
            }
        };
        where_it_landed.insert(index, id);
        Some(id)
    }

    /// Follows a route of turns down from the root.
    fn walk(&self, path: &[Side]) -> Option<NodeId> {
        let mut current = self.root()?;
        for side in path {
            current = self.node(current).ok()?.get_split()?.child(*side);
        }
        Some(current)
    }
}

impl<'de, Tab: Deserialize<'de>> Deserialize<'de> for Tree<Tab> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let TreeIn {
            root,
            focused,
            nodes,
            focused_node,
        } = TreeIn::deserialize(deserializer)?;

        let mut tree = Tree::default();

        match root {
            // Current form.
            Some(root) => {
                tree.root = match root {
                    // A leaf at the root may legitimately be empty — that is an empty dock,
                    // and the only place `validate` allows one — so it bypasses the pruning
                    // that `build` applies below the root.
                    NodeIn::Leaf {
                        tabs,
                        active,
                        prev_active,
                        scroll,
                        collapsed,
                    } => Some(tree.build_leaf(tabs, active, prev_active, scroll, collapsed)),
                    split => tree.build(split),
                };
                tree.focused_node = focused.and_then(|path| tree.walk(&path));
            }
            // Pre-arena form. An absent `root` and an empty `nodes` both mean "no tree",
            // and both land here with the same (empty) result — there is nothing to
            // disambiguate.
            None => {
                let mut nodes: Vec<Option<LegacyNode<Tab>>> = nodes.into_iter().map(Some).collect();
                let mut where_it_landed = HashMap::new();
                tree.root = tree.build_legacy(&mut nodes, 0, &mut where_it_landed);
                tree.focused_node =
                    focused_node.and_then(|index| where_it_landed.get(&index.0).copied());
            }
        }

        // The collapsing bookkeeping is derived, so it is recomputed rather than read, for
        // two independent reasons:
        //
        //  * the reading above *repairs* the shape — an empty leaf below the root is dropped,
        //    a split left with one child is replaced by it — so the numbers a file states
        //    describe a tree that this one is not. The stale-count bug that the in-memory
        //    sweeps were fixed for would simply have arrived through the reader instead;
        //  * files on disk were written by builds whose sweeps got these numbers wrong, and
        //    trusting them keeps a fixed bug alive for as long as the file exists.
        //
        // Nothing is lost by ignoring them: every one of them follows from `Leaf::collapsed`,
        // which is read. The writer still emits them, so files stay loadable by older builds.
        tree.recompute_collapsed();

        // Focus is the one field a stored layout can get wrong without the shape being
        // wrong: it may name a node that the file's own repairs dropped, or a split.
        // Dropping it is honest — "no leaf is focused" is a state the tree already has.
        if !tree
            .focused_node
            .is_some_and(|id| tree.node(id).is_ok_and(Node::is_leaf))
        {
            tree.focused_node = None;
        }

        Ok(tree)
    }
}

// ----------------------------------------------------------------------------
// The dock around the trees
//
// On disk a dock is a flat vector of surfaces with the main one at position 0, plus a numeric
// index into that vector. In memory it is no longer shaped like that at all: the main surface
// is its own field and windows live in their own vector, so that "there is a main surface" and
// "the main surface is not a window" stop being things anyone has to check.
//
// That difference is the whole reason both directions are written by hand. The stored form is
// kept exactly as it was — the corpus of saved layouts has to keep loading, and files written
// now have to keep loading in older builds — so this module is the one place that knows the
// translation, and `SurfaceIndex` itself no longer carries a number at all.
//
// A file can also simply be wrong: focus naming a surface that is not there, a hole where the
// main surface should be, a window stored at position 0. Reading repairs all three, the same
// way tree reading above repairs a split that lost a child.

/// The stored form of a surface: position 0 is the main one, later positions are windows,
/// and `Empty` is a hole left by a closed window.
///
/// Deliberately a separate type from anything in the model. It is the *format*, and it must
/// not follow the model when the model changes shape.
#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
enum WireSurface<Tab> {
    Empty,
    Main(Tree<Tab>),
    Window(Tree<Tab>, crate::WindowState),
}

/// The same, borrowed, for writing — so saving a layout does not clone every tree.
///
/// Serde renders this exactly as [`WireSurface`]: same variant names, same arities.
#[derive(Serialize)]
#[serde(bound(serialize = "Tab: Serialize"))]
enum WireSurfaceRef<'a, Tab> {
    Empty,
    Main(&'a Tree<Tab>),
    Window(&'a Tree<Tab>, &'a crate::WindowState),
}

/// The stored form of a surface index: a position in the flat vector above.
///
/// A newtype, because that is what `SurfaceIndex` used to be and RON writes it as `(0)`.
#[derive(Serialize, Deserialize, Clone, Copy)]
struct WireSurfaceIndex(usize);

impl WireSurfaceIndex {
    /// The stored position of `index`: main is 0, window *n* is *n + 1*.
    fn of(index: crate::SurfaceIndex) -> Self {
        match index {
            crate::SurfaceIndex::Main => Self(0),
            crate::SurfaceIndex::Window(window) => Self(window.0 + 1),
        }
    }

    /// What a stored position names.
    fn resolve(self) -> crate::SurfaceIndex {
        match self.0 {
            0 => crate::SurfaceIndex::Main,
            position => crate::SurfaceIndex::window(position - 1),
        }
    }
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "Tab: Deserialize<'de>"))]
struct DockIn<Tab> {
    surfaces: Vec<WireSurface<Tab>>,
    #[serde(default = "none")]
    focused_surface: Option<WireSurfaceIndex>,
}

#[derive(Serialize)]
#[serde(bound(serialize = "Tab: Serialize"))]
struct DockOut<'a, Tab> {
    surfaces: Vec<WireSurfaceRef<'a, Tab>>,
    focused_surface: Option<WireSurfaceIndex>,
}

impl<Tab: Serialize> Serialize for crate::DockState<Tab> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Translations are not state a layout carries; they were `#[serde(skip)]` under the
        // derived impl and are simply absent here, which renders the same.
        let mut surfaces = Vec::with_capacity(self.surfaces_count());
        for (_, surface) in self.iter_surfaces_indexed() {
            surfaces.push(match surface {
                crate::SurfaceRef::Empty => WireSurfaceRef::Empty,
                crate::SurfaceRef::Main(tree) => WireSurfaceRef::Main(tree),
                crate::SurfaceRef::Window(tree, state) => WireSurfaceRef::Window(tree, state),
            });
        }
        DockOut {
            surfaces,
            focused_surface: self.focused_surface.map(WireSurfaceIndex::of),
        }
        .serialize(serializer)
    }
}

impl<'de, Tab: Deserialize<'de>> Deserialize<'de> for crate::DockState<Tab> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let DockIn {
            surfaces,
            focused_surface,
        } = DockIn::deserialize(deserializer)?;

        let mut stored = surfaces.into_iter();

        // Position 0 is the main surface. A file that has nothing there, or a hole, or — this
        // was reachable under the old model — a *window* there, all resolve to "the main
        // surface holds this tree, or an empty one". None of them can shift another surface's
        // position, so no index a file carries is invalidated by the repair.
        let main = match stored.next() {
            None | Some(WireSurface::Empty) => Tree::default(),
            Some(WireSurface::Main(tree)) => tree,
            Some(WireSurface::Window(tree, _)) => tree,
        };

        // A `Main` stored at a window position was expressible in the old model and is not in
        // this one. Its tree is kept as a window rather than dropped: losing tabs silently is
        // worse than a window that arrives without a remembered position.
        let windows = stored
            .map(|surface| match surface {
                WireSurface::Empty => None,
                WireSurface::Window(tree, state) => Some((tree, state)),
                WireSurface::Main(tree) => Some((tree, crate::WindowState::default())),
            })
            .collect();

        let mut state = crate::DockState {
            main,
            windows,
            // Focus into a surface that the file does not actually contain is dropped by
            // `normalize_surfaces` below, exactly as a focus route that leads nowhere is
            // dropped inside a tree: "nothing is focused" is a state the dock already has.
            focused_surface: focused_surface.map(WireSurfaceIndex::resolve),
            // Translations are not state a layout carries; a freshly read dock gets the
            // defaults, as it did under the derived impl.
            translations: crate::Translations::default(),
        };
        state.normalize_surfaces();

        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use crate::{DockState, Node, Split, SurfaceIndex, TabIndex, Tree};

    fn shape(tree: &Tree<String>) -> Vec<(usize, Vec<String>)> {
        tree.breadth_first()
            .into_iter()
            .map(|id| match &tree[id] {
                Node::Leaf(leaf) => (0, leaf.iter_tabs().cloned().collect()),
                Node::Vertical(_) => (1, vec![]),
                Node::Horizontal(_) => (2, vec![]),
            })
            .collect()
    }

    fn sample() -> Tree<String> {
        let mut tree = Tree::new(vec!["a".to_string(), "b".to_string()]);
        let root = tree.root().unwrap();
        let [left, right] = tree.split_right(root, 0.25, vec!["c".to_string()]);
        let _ = tree.split_below(right, 0.75, vec!["d".to_string()]);
        tree.set_active_tab(left, TabIndex(1)).unwrap();
        tree.set_focused_node(left);
        tree
    }

    #[test]
    fn round_trip_preserves_shape_focus_and_fractions() {
        let tree = sample();
        let json = serde_json::to_string(&tree).unwrap();
        let back: Tree<String> = serde_json::from_str(&json).unwrap();

        assert_eq!(back.validate(), Ok(()));
        assert_eq!(shape(&back), shape(&tree));
        assert_eq!(back.num_tabs(), tree.num_tabs());

        // Focus and the active tab are addressed by identity in memory and by position on
        // disk; this is the assertion that the translation is lossless.
        let focused = back.focused_leaf().expect("focus survived the round trip");
        assert_eq!(
            back.leaf(focused).unwrap().iter_tabs().collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(
            back.leaf(focused).unwrap().active_index(),
            Some(TabIndex(1))
        );

        let fractions: Vec<f32> = back
            .breadth_first()
            .into_iter()
            .filter_map(|id| back[id].get_split().map(|split| split.fraction))
            .collect();
        assert_eq!(fractions, vec![0.25, 0.75]);
    }

    /// The gate that matters for users: layouts written before the arena still load.
    ///
    /// This fixture is the old on-disk shape verbatim — an implicit binary heap with holes
    /// spelled `Empty`, focus as an index into it, and the geometry fields that used to be
    /// serialized alongside the model (`rect`, `viewport`). Reading it must produce the
    /// tree the file describes and ignore the geometry.
    #[test]
    fn reads_the_pre_arena_heap_format() {
        // Heap: 0 = horizontal split, 1 = leaf [a, b] (active b), 2 = leaf [c].
        let legacy = r#"{
            "nodes": [
                { "Horizontal": {
                    "rect": { "min": {"x": 0.0, "y": 24.0}, "max": {"x": 871.0, "y": 878.0} },
                    "fraction": 0.75,
                    "fully_collapsed": false,
                    "collapsed_leaf_count": 0
                }},
                { "Leaf": {
                    "rect": { "min": {"x": 0.0, "y": 24.0}, "max": {"x": 656.0, "y": 878.0} },
                    "viewport": { "min": {"x": 0.0, "y": 48.0}, "max": {"x": 656.0, "y": 878.0} },
                    "tabs": ["a", "b"],
                    "active": 1,
                    "scroll": 0.0,
                    "collapsed": false
                }},
                { "Leaf": {
                    "tabs": ["c"],
                    "active": 0,
                    "scroll": 0.0,
                    "collapsed": false
                }}
            ],
            "focused_node": 2,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(legacy).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(
            shape(&tree),
            vec![
                (2, vec![]),
                (0, vec!["a".to_string(), "b".to_string()]),
                (0, vec!["c".to_string()]),
            ]
        );
        assert_eq!(tree.num_tabs(), 3);

        let root = tree.root().unwrap();
        assert_eq!(tree[root].get_split().unwrap().fraction, 0.75);
        let [left, right] = tree.children(root).unwrap();
        assert_eq!(tree.leaf(left).unwrap().active_index(), Some(TabIndex(1)));
        assert_eq!(
            tree.focused_leaf(),
            Some(right),
            "the old focus index names the same leaf it used to"
        );
    }

    /// Trailing `Empty` slots and holes are what the old format was mostly made of; they
    /// must disappear rather than turn into nodes.
    #[test]
    fn pre_arena_holes_do_not_become_nodes() {
        let legacy = r#"{
            "nodes": [
                { "Vertical": { "fraction": 0.5, "fully_collapsed": false, "collapsed_leaf_count": 0 }},
                { "Leaf": { "tabs": ["a"], "active": 0, "scroll": 0.0, "collapsed": false }},
                { "Leaf": { "tabs": ["b"], "active": 0, "scroll": 0.0, "collapsed": false }},
                "Empty", "Empty", "Empty", "Empty"
            ],
            "focused_node": null,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(legacy).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(tree.len(), 3, "three live nodes, not seven slots");
        assert_eq!(tree.focused_leaf(), None);
    }

    /// A split that lost one child in the file is repaired the same way the in-memory tree
    /// repairs it, instead of loading a split with a missing side.
    #[test]
    fn pre_arena_half_split_is_repaired() {
        let legacy = r#"{
            "nodes": [
                { "Vertical": { "fraction": 0.5, "fully_collapsed": false, "collapsed_leaf_count": 0 }},
                { "Leaf": { "tabs": ["a"], "active": 0, "scroll": 0.0, "collapsed": false }},
                "Empty"
            ],
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(legacy).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(tree.len(), 1);
        assert!(tree.root_node().unwrap().is_leaf());
    }

    /// An empty dock survives a round trip as an empty dock, not as `null` turning into a
    /// tree with a phantom node.
    #[test]
    fn empty_trees_round_trip() {
        let empty: Tree<String> = Tree::default();
        let back: Tree<String> =
            serde_json::from_str(&serde_json::to_string(&empty).unwrap()).unwrap();
        assert!(back.is_empty());
        assert_eq!(back.validate(), Ok(()));

        let root_only = Tree::<String>::new(vec![]);
        let back: Tree<String> =
            serde_json::from_str(&serde_json::to_string(&root_only).unwrap()).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back.validate(), Ok(()));
    }

    /// Focus that a file cannot honour (it names a split) is dropped rather than stored as
    /// a lie: `focused_leaf()` promises a leaf.
    #[test]
    fn focus_naming_a_split_is_dropped_on_load() {
        let mut tree = Tree::new(vec!["a".to_string()]);
        let root = tree.root().unwrap();
        let _ = tree.split(root, Split::Below, 0.5, Node::leaf("b".to_string()));
        let json = serde_json::to_string(&tree).unwrap();
        // Point focus at the root, which is the split.
        let json = json.replace("\"focused\":[\"Right\"]", "\"focused\":[]");

        let back: Tree<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.focused_leaf(), None);
        assert_eq!(back.validate(), Ok(()));
    }

    /// Found by the `tree_persist` fuzz target: a file describing a leaf with no tabs below
    /// the root loaded into a tree that failed its own oracle (`EmptyLeaf`).
    ///
    /// The state is unreachable in memory — removing the last tab collapses the leaf — but
    /// nothing stopped it from arriving in a file, and `Deserialize` returning `Ok` is a
    /// promise that what came back is well-formed. It is repaired the way the pre-arena
    /// reader already repairs a half-split: the empty side is dropped and the split collapses
    /// onto the surviving one.
    #[test]
    fn an_empty_leaf_below_the_root_is_dropped_on_load() {
        let json = r#"{
            "root": { "Horizontal": {
                "fraction": 0.5,
                "fully_collapsed": false,
                "collapsed_leaf_count": 0,
                "children": [
                    { "Leaf": { "tabs": [], "active": 0, "scroll": 0.0, "collapsed": false }},
                    { "Leaf": { "tabs": ["a"], "active": 0, "scroll": 0.0, "collapsed": false }}
                ]
            }},
            "focused": null,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(json).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(
            tree.len(),
            1,
            "the split collapsed onto its surviving child"
        );
        assert_eq!(tree.num_tabs(), 1, "the tab that was there is still there");
        assert!(tree.root_node().unwrap().is_leaf());
    }

    /// The same hole in the pre-arena reader — and this is the one that can actually be on a
    /// user's disk, since those files are the ones written by older versions.
    #[test]
    fn an_empty_leaf_in_the_pre_arena_heap_is_dropped_on_load() {
        let legacy = r#"{
            "nodes": [
                { "Vertical": { "fraction": 0.5, "fully_collapsed": false, "collapsed_leaf_count": 0 }},
                { "Leaf": { "tabs": [], "active": 0, "scroll": 0.0, "collapsed": false }},
                { "Leaf": { "tabs": ["a"], "active": 0, "scroll": 0.0, "collapsed": false }}
            ],
            "focused_node": 1,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(legacy).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.num_tabs(), 1);
        assert_eq!(
            tree.focused_leaf(),
            None,
            "focus pointed at the leaf that was dropped, so there is nothing to focus"
        );
    }

    /// Found by the `tree_persist` fuzz target: a stored dock whose `focused_surface` names a
    /// surface the file does not contain used to load as-is.
    ///
    /// Nothing rejects it later — `focused_leaf()` indexes that surface and panics — so the
    /// file is repaired on the way in: nothing is focused.
    #[test]
    fn saved_focus_into_a_surface_that_is_not_there_is_dropped() {
        let json = r#"{
            "surfaces": [
                { "Main": { "root": { "Leaf": { "tabs": ["a"], "active": 0 }}, "focused": null }}
            ],
            "focused_surface": 3
        }"#;

        let dock_state: DockState<String> = serde_json::from_str(json).unwrap();
        assert_eq!(dock_state.validate(), Ok(()));
        assert_eq!(dock_state.focused_leaf(), None);
    }

    /// The other half: a file with no main surface at all, or with a hole where it should be.
    /// Indexing, `main_surface()` and focus resolution all assume it is there.
    #[test]
    fn a_saved_dock_without_a_main_surface_gets_one() {
        for json in [
            r#"{ "surfaces": [], "focused_surface": null }"#,
            r#"{ "surfaces": ["Empty"], "focused_surface": null }"#,
        ] {
            let dock_state: DockState<String> = serde_json::from_str(json).unwrap();
            assert_eq!(dock_state.validate(), Ok(()), "for {json}");
            assert_eq!(dock_state.main_surface().num_tabs(), 0);
            assert!(dock_state.is_surface_valid(SurfaceIndex::main()));
        }
    }

    /// And the ordinary case still round-trips: surfaces, their windows and the focus between
    /// them survive a save.
    #[test]
    fn dock_state_round_trips_with_windows() {
        let mut dock_state = DockState::new(vec!["a".to_string()]);
        let window = dock_state.add_window(vec!["b".to_string()]);
        let root = dock_state[window].root().unwrap();
        dock_state.set_focused_node_and_surface(crate::NodePath {
            surface: window,
            node: root,
        });

        let back: DockState<String> =
            serde_json::from_str(&serde_json::to_string(&dock_state).unwrap()).unwrap();

        assert_eq!(back.validate(), Ok(()));
        assert_eq!(back.surfaces_count(), 2);
        assert_eq!(
            back.focused_leaf().map(|path| path.surface),
            Some(window),
            "focus stayed in the window it was in"
        );
    }

    /// The stored numbering is frozen, and it is no longer the numbering the dock uses.
    ///
    /// In memory the main surface is a field and windows count from zero; on disk everything is
    /// one flat vector with main at position 0. Old files have to keep loading and new files
    /// have to keep the same positions, so this pins both directions at once — including a
    /// hole, which is the case where an off-by-one would silently move a window rather than
    /// fail loudly.
    #[test]
    fn stored_positions_survive_the_move_of_main_out_of_the_vector() {
        // `"focused": []` is the route to the root — the window's own leaf is focused, so that
        // `focused_leaf()` below can report which surface focus landed in.
        let leaf = |tab: &str, focused: &str| {
            format!(
                r#"{{ "root": {{ "Leaf": {{ "tabs": ["{tab}"], "active": 0 }}}}, "focused": {focused} }}"#
            )
        };
        let window_state = r#"{ "next_position": null, "next_size": null,
                                "expanded_height": null, "new": false, "minimized": false }"#;
        let json = format!(
            r#"{{ "surfaces": [
                    {{ "Main": {} }},
                    "Empty",
                    {{ "Window": [{}, {}] }}
                 ],
                 "focused_surface": 2 }}"#,
            leaf("main", "null"),
            leaf("second", "[]"),
            window_state
        );

        let dock_state: DockState<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(dock_state.validate(), Ok(()));

        // Position 2 on disk is window 1 in memory; position 1 is the hole between them.
        assert_eq!(dock_state.main_surface().num_tabs(), 1);
        assert!(
            !dock_state.is_surface_valid(SurfaceIndex::window(0)),
            "the hole stayed a hole"
        );
        assert_eq!(
            dock_state[SurfaceIndex::window(1)]
                .tabs()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["second".to_string()]
        );
        assert_eq!(
            dock_state.focused_leaf().map(|path| path.surface),
            Some(SurfaceIndex::window(1)),
            "focus followed the window, not the raw number"
        );

        // And back out at the very same positions, hole included.
        let written: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&dock_state).unwrap()).unwrap();
        let surfaces = written["surfaces"].as_array().unwrap();
        assert_eq!(surfaces.len(), 3);
        assert!(
            surfaces[0].get("Main").is_some(),
            "main is written at position 0"
        );
        assert_eq!(surfaces[1], serde_json::json!("Empty"));
        assert!(
            surfaces[2].get("Window").is_some(),
            "window 1 is written at position 2"
        );
        assert_eq!(written["focused_surface"], serde_json::json!(2));
    }

    /// The collapsing numbers in a file are a *claim*, and loading must not believe it.
    ///
    /// Both directions are pinned, because a reader that trusts the file is wrong in both:
    /// a file that understates the count leaves a collapsed dock rendering at full height,
    /// one that overstates it leaves an expanded dock reserving rows for nothing. The
    /// numbers follow from `Leaf::collapsed`, which is the only thing here worth reading.
    ///
    /// Why a file can say something else at all: builds before the derived-counts fix wrote
    /// stale numbers, and those files are on disk now.
    #[test]
    fn stored_collapsed_counts_are_recomputed_rather_than_believed() {
        let json = |leaf_collapsed: bool, claim: i32| {
            format!(
                r#"{{
                    "root": {{ "Vertical": {{
                        "fraction": 0.5,
                        "fully_collapsed": false,
                        "collapsed_leaf_count": {claim},
                        "children": [
                            {{ "Leaf": {{ "tabs": ["a"], "active": 0, "collapsed": {leaf_collapsed} }}}},
                            {{ "Leaf": {{ "tabs": ["b"], "active": 0, "collapsed": {leaf_collapsed} }}}}
                        ]
                    }}}},
                    "focused": null,
                    "collapsed": false,
                    "collapsed_leaf_count": {claim}
                }}"#
            )
        };

        // Understated: two collapsed leaves, the file insists there are none.
        let tree: Tree<String> = serde_json::from_str(&json(true, 0)).unwrap();
        let root = tree.root().unwrap();
        assert_eq!(
            tree[root].collapsed_leaf_count(),
            2,
            "a vertical split stacks its children, so two collapsed leaves are two rows"
        );
        assert!(tree[root].is_collapsed(), "both children are collapsed");
        assert_eq!(
            tree.collapsed_leaf_count(),
            2,
            "and the tree mirrors its root — this is the number the window height uses"
        );
        assert!(tree.is_collapsed());

        // Overstated: nothing is collapsed, the file claims seven rows.
        let tree: Tree<String> = serde_json::from_str(&json(false, 7)).unwrap();
        let root = tree.root().unwrap();
        assert_eq!(tree[root].collapsed_leaf_count(), 0);
        assert!(!tree[root].is_collapsed());
        assert_eq!(tree.collapsed_leaf_count(), 0);
        assert!(!tree.is_collapsed());
    }

    /// The pre-arena reader gets the same treatment — and it is the one whose files are
    /// actually on users' disks, written by exactly the builds that got the counts wrong.
    #[test]
    fn the_pre_arena_reader_recomputes_the_collapsed_counts_too() {
        let legacy = r#"{
            "nodes": [
                { "Vertical": { "fraction": 0.5, "fully_collapsed": false, "collapsed_leaf_count": 0 }},
                { "Leaf": { "tabs": ["a"], "active": 0, "collapsed": true }},
                { "Leaf": { "tabs": ["b"], "active": 0, "collapsed": true }}
            ],
            "focused_node": null,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(legacy).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert_eq!(tree.collapsed_leaf_count(), 2);
        assert!(tree.is_collapsed());
    }

    /// The sharp case: reading *repairs* the shape, so a count read from the file would
    /// describe a tree that no longer exists.
    ///
    /// Here the file's own numbers are self-consistent — two collapsed leaves under a
    /// vertical split, two rows — but one of those leaves is empty, and an empty leaf below
    /// the root is dropped on the way in. Believing the file leaves the surviving single
    /// leaf claiming the height of two.
    #[test]
    fn a_leaf_the_reader_drops_leaves_no_trace_in_the_counts() {
        let json = r#"{
            "root": { "Vertical": {
                "fraction": 0.5,
                "fully_collapsed": true,
                "collapsed_leaf_count": 2,
                "children": [
                    { "Leaf": { "tabs": [], "active": 0, "collapsed": true }},
                    { "Leaf": { "tabs": ["a"], "active": 0, "collapsed": true }}
                ]
            }},
            "focused": null,
            "collapsed": true,
            "collapsed_leaf_count": 2
        }"#;

        let tree: Tree<String> = serde_json::from_str(json).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert!(
            tree.root_node().unwrap().is_leaf(),
            "the split collapsed onto its surviving child"
        );
        assert_eq!(
            tree.collapsed_leaf_count(),
            1,
            "one collapsed leaf survived, so one row — not the two the file described"
        );
        assert!(tree.is_collapsed());
    }

    /// A file made *only* of empty leaves has nothing to repair onto: the result is an empty
    /// dock, not a tree of leaves nobody can put a tab into.
    #[test]
    fn a_tree_of_only_empty_leaves_loads_as_an_empty_dock() {
        let json = r#"{
            "root": { "Vertical": {
                "fraction": 0.5,
                "fully_collapsed": false,
                "collapsed_leaf_count": 0,
                "children": [
                    { "Leaf": { "tabs": [], "active": 0, "scroll": 0.0, "collapsed": false }},
                    { "Leaf": { "tabs": [], "active": 0, "scroll": 0.0, "collapsed": false }}
                ]
            }},
            "focused": null,
            "collapsed": false,
            "collapsed_leaf_count": 0
        }"#;

        let tree: Tree<String> = serde_json::from_str(json).unwrap();
        assert_eq!(tree.validate(), Ok(()));
        assert!(tree.is_empty());
    }
}
