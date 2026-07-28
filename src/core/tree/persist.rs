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

    // Written by both.
    #[serde(default)]
    collapsed: bool,
    #[serde(default)]
    collapsed_leaf_count: i32,
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
    Vertical {
        fraction: f32,
        #[serde(default)]
        fully_collapsed: bool,
        #[serde(default)]
        collapsed_leaf_count: i32,
        children: [Box<NodeIn<Tab>>; 2],
    },
    Horizontal {
        fraction: f32,
        #[serde(default)]
        fully_collapsed: bool,
        #[serde(default)]
        collapsed_leaf_count: i32,
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

#[derive(Deserialize)]
struct LegacySplit {
    fraction: f32,
    #[serde(default)]
    fully_collapsed: bool,
    #[serde(default)]
    collapsed_leaf_count: i32,
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
    fn adopt_split(
        &mut self,
        vertical: bool,
        children: [NodeId; 2],
        fraction: f32,
        fully_collapsed: bool,
        collapsed_leaf_count: i32,
    ) -> NodeId {
        let split = SplitNode::new(children, fraction, fully_collapsed, collapsed_leaf_count);
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

    /// Builds a subtree of the current form, returning the id it got.
    fn build(&mut self, node: NodeIn<Tab>) -> NodeId {
        let vertical = matches!(node, NodeIn::Vertical { .. });
        match node {
            NodeIn::Leaf {
                tabs,
                active,
                prev_active,
                scroll,
                collapsed,
            } => {
                let leaf = LeafNode::from_persisted(tabs, active, prev_active, scroll, collapsed);
                self.adopt(Node::Leaf(leaf))
            }
            NodeIn::Vertical {
                fraction,
                fully_collapsed,
                collapsed_leaf_count,
                children,
            }
            | NodeIn::Horizontal {
                fraction,
                fully_collapsed,
                collapsed_leaf_count,
                children,
            } => {
                let [left, right] = children;
                let left = self.build(*left);
                let right = self.build(*right);
                self.adopt_split(
                    vertical,
                    [left, right],
                    fraction,
                    fully_collapsed,
                    collapsed_leaf_count,
                )
            }
        }
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
                let leaf = LeafNode::from_persisted(tabs, active, prev_active, scroll, collapsed);
                self.adopt(Node::Leaf(leaf))
            }
            LegacyNode::Vertical(split) | LegacyNode::Horizontal(split) => {
                let vertical = vertical_split;
                let left = self.build_legacy(nodes, index * 2 + 1, where_it_landed);
                let right = self.build_legacy(nodes, index * 2 + 2, where_it_landed);
                match (left, right) {
                    (Some(left), Some(right)) => self.adopt_split(
                        vertical,
                        [left, right],
                        split.fraction,
                        split.fully_collapsed,
                        split.collapsed_leaf_count,
                    ),
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
            collapsed,
            collapsed_leaf_count,
        } = TreeIn::deserialize(deserializer)?;

        let mut tree = Tree::default();
        tree.set_collapsed(collapsed);
        tree.set_collapsed_leaf_count(collapsed_leaf_count);

        match root {
            // Current form.
            Some(root) => {
                tree.root = Some(tree.build(root));
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

#[cfg(test)]
mod tests {
    use crate::{Node, Split, TabIndex, Tree};

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
}
