//! Binary tree representing the relationships between [`Node`]s.
//!
//! # Implementation details
//!
//! Nodes live in a generational arena ([`arena`]) and are addressed by [`NodeId`]. The
//! shape is carried by explicit links: every node knows its parent, every split knows its
//! two children. Nothing is addressed by position, so a structural edit renames nothing
//! and an id taken before the edit still names the same node afterwards.
//!
//! The previous representation was an implicit binary heap in a `Vec` (children of *n* at
//! *2n + 1* and *2n + 2*, holes filled with `Node::Empty`). It made every address
//! positional — which is the shared root of the `move_tab` out-of-bounds fix, the
//! `prev_active` fix, and of layouts that stored hundreds of empty slots.

/// Generational slot storage for nodes.
mod arena;

/// Iterates over all tabs in a [`Tree`].
pub mod tab_iter;

/// Identifies a tab within a [`Node`].
pub mod tab_index;

/// Represents an abstract node of a [`Tree`].
pub mod node;

/// Stable identity of a node inside a [`Tree`].
pub mod node_id;

/// Reading and writing the persisted shape of a [`Tree`].
#[cfg(feature = "serde")]
pub mod persist;

/// Read-only structural oracle: are a tree's invariants intact?
pub mod validate;

/// Rewriting the shape of a subtree out of the nodes it already has.
pub(crate) mod regroup;

/// Transposing the grouping around a crossing — see the module for the picture.
pub(crate) mod transpose;

use std::{
    cmp::max,
    collections::{HashSet, VecDeque},
    fmt,
    ops::{Index, IndexMut},
};

pub use node::Fold;
pub use node::LeafNode;
pub use node::Node;
pub use node::RowNode;
pub use node::Share;
pub use node::TabId;
pub use node_id::{ChildIndex, GapIndex, GapPath, NodeId, NodePath, RowGap};
pub use tab_index::{TabIndex, TabPath};
pub use tab_iter::TabIter;
pub use validate::{DockViolation, SurfaceViolation, TreeViolation};

use crate::core::geom::Rect;
use crate::core::{Error, Result, SurfaceIndex};
use arena::{Arena, NodeEntry};

// ----------------------------------------------------------------------------

/// Direction in which a new node is created relatively to the parent node at which the split occurs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[allow(missing_docs)]
pub enum Split {
    Left,
    Right,
    Above,
    Below,
}

impl Split {
    /// Returns whether the split is vertical.
    pub const fn is_top_bottom(self) -> bool {
        matches!(self, Split::Above | Split::Below)
    }

    /// Returns whether the split is horizontal.
    pub const fn is_left_right(self) -> bool {
        matches!(self, Split::Left | Split::Right)
    }
}

/// Specify how a tab should be added to a Node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabInsert {
    /// Split the node in the given direction.
    Split(Split),

    /// Insert the tab at the given index.
    Insert(TabIndex),

    /// Append the tab to the node.
    Append,
}

/// The destination for a tab which is being moved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TabDestination {
    /// Move to a new window with this rect.
    Window(Rect),

    /// Move to an existing node with this insertion.
    Node(NodePath, TabInsert),

    /// Move to an empty surface.
    EmptySurface(SurfaceIndex),
}

impl From<(NodePath, TabInsert)> for TabDestination {
    fn from(value: (NodePath, TabInsert)) -> TabDestination {
        TabDestination::Node(value.0, value.1)
    }
}

impl From<SurfaceIndex> for TabDestination {
    fn from(value: SurfaceIndex) -> TabDestination {
        TabDestination::EmptySurface(value)
    }
}

impl TabDestination {
    /// Returns if this tab destination is a [`Window`](TabDestination::Window).
    pub fn is_window(&self) -> bool {
        matches!(self, Self::Window(_))
    }
}

/// Binary tree representing the relationships between [`Node`]s.
///
/// Nodes are addressed by [`NodeId`], which is stable across every structural operation.
/// For "Horizontal" nodes the first child is the left one and the second the right one;
/// for "Vertical" nodes the first is the top one and the second the bottom one.
#[derive(Clone)]
pub struct Tree<Tab> {
    nodes: Arena<Tab>,

    /// The node everything hangs off, or `None` for an empty tree.
    root: Option<NodeId>,

    focused_node: Option<NodeId>,

    /// Whether all subnodes of the tree are collapsed.
    collapsed: bool,
    collapsed_leaf_count: i32,
}

impl<Tab> fmt::Debug for Tree<Tab> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tree").finish_non_exhaustive()
    }
}

impl<Tab> Default for Tree<Tab> {
    fn default() -> Self {
        Self {
            nodes: Arena::default(),
            root: None,
            focused_node: None,
            collapsed: false,
            collapsed_leaf_count: 0,
        }
    }
}

impl<Tab> Index<NodeId> for Tree<Tab> {
    type Output = Node<Tab>;

    #[inline]
    #[track_caller]
    fn index(&self, id: NodeId) -> &Self::Output {
        &self
            .nodes
            .get(id)
            .unwrap_or_else(|| panic!("no node {id} in this tree"))
            .node
    }
}

impl<Tab> IndexMut<NodeId> for Tree<Tab> {
    #[inline]
    #[track_caller]
    fn index_mut(&mut self, id: NodeId) -> &mut Self::Output {
        &mut self
            .nodes
            .get_mut(id)
            .unwrap_or_else(|| panic!("no node {id} in this tree"))
            .node
    }
}

impl<Tab> Tree<Tab> {
    /// Creates a new [`Tree`] with given `Vec` of `Tab`s in its root node.
    ///
    /// An **empty** `tabs` gives a tree with no root, which is what every other route to an
    /// empty dock produces (closing the last tab, [`retain_tabs`](Self::retain_tabs),
    /// [`filter_tabs`](Self::filter_tabs) — all of them go through
    /// [`remove_leaf`](Self::remove_leaf), which empties the tree). A root leaf holding no tabs
    /// used to be a second shape of the same state, and the two did not behave alike:
    /// [`is_empty`](Self::is_empty) asks about the root, so the second answered "not empty",
    /// and the renderer branches on that — the empty root leaf drew a strip of empty tab bar
    /// and offered a leaf-sized drop target where the empty dock offers its whole area.
    ///
    /// Gate: `tests/an_empty_dock_has_one_shape.rs`.
    pub fn new(tabs: Vec<Tab>) -> Self {
        let mut tree = Self::default();
        if !tabs.is_empty() {
            tree.root = Some(tree.nodes.insert(NodeEntry {
                parent: None,
                node: Node::leaf_with(tabs),
            }));
        }
        tree
    }

    /// The root node of the tree, or `None` if the tree holds no nodes at all.
    #[inline]
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }

    /// Whether `id` still names a live node of this tree.
    #[inline]
    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains(id)
    }

    /// The split `id` hangs off, or `None` if `id` is the root (or not in this tree).
    #[inline]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(id).and_then(|entry| entry.parent)
    }

    /// The children of `id`, in order, or `None` if it is a leaf (or not in this tree).
    ///
    /// See [`RowNode::children`] for why this is a slice; [`children_pair`](Self::children_pair)
    /// is the spelling for callers that need exactly two.
    #[inline]
    pub fn children(&self, id: NodeId) -> Option<&[NodeId]> {
        self.node(id).ok()?.get_row().map(RowNode::children)
    }

    /// The two children of `id`, or `None` if it is a leaf (or not in this tree).
    ///
    /// See [`RowNode::children_pair`]: a caller of this one is a caller that still needs a
    /// split to hold exactly two children.
    #[inline]
    pub fn children_pair(&self, id: NodeId) -> Option<[NodeId; 2]> {
        self.node(id).ok()?.get_row().map(RowNode::children_pair)
    }

    /// Immutably borrows the node `id` names.
    pub fn node(&self, id: NodeId) -> Result<&Node<Tab>> {
        self.nodes
            .get(id)
            .map(|entry| &entry.node)
            .ok_or(Error::InvalidNode)
    }

    /// Mutably borrows the node `id` names.
    pub fn node_mut(&mut self, id: NodeId) -> Result<&mut Node<Tab>> {
        self.nodes
            .get_mut(id)
            .map(|entry| &mut entry.node)
            .ok_or(Error::InvalidNode)
    }

    /// Immutably borrows a leaf node.
    ///
    /// Returns `Err` if the id is stale or the node is not a leaf.
    pub fn leaf(&self, node: NodeId) -> Result<&LeafNode<Tab>> {
        self.node(node)?.get_leaf().ok_or(Error::NonLeafNode)
    }

    /// Mutably borrows a leaf node.
    ///
    /// Returns `Err` if the id is stale or the node is not a leaf.
    pub fn leaf_mut(&mut self, node: NodeId) -> Result<&mut LeafNode<Tab>> {
        self.node_mut(node)?
            .get_leaf_mut()
            .ok_or(Error::NonLeafNode)
    }

    /// Returns the number of live nodes in the [`Tree`].
    #[inline]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` if the tree holds no nodes at all.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Returns an [`Iterator`] of the nodes of this tree, in unspecified order.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &Node<Tab>> {
        self.nodes.iter().map(|(_, entry)| &entry.node)
    }

    /// Returns a mutable [`Iterator`] of the nodes of this tree, in unspecified order.
    #[inline]
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Node<Tab>> {
        self.nodes.iter_mut().map(|(_, entry)| &mut entry.node)
    }

    /// Returns an [`Iterator`] of the nodes of this tree with their ids, in unspecified order.
    #[inline]
    pub fn iter_indexed(&self) -> impl Iterator<Item = (NodeId, &Node<Tab>)> {
        self.nodes.iter().map(|(id, entry)| (id, &entry.node))
    }

    /// Returns a mutable [`Iterator`] of the nodes of this tree with their ids, in
    /// unspecified order.
    #[inline]
    pub fn iter_mut_indexed(&mut self) -> impl Iterator<Item = (NodeId, &mut Node<Tab>)> {
        self.nodes
            .iter_mut()
            .map(|(id, entry)| (id, &mut entry.node))
    }

    /// Every node of the tree, parents before their children.
    ///
    /// The layout pass depends on that order: a node's rectangle is cut out of its
    /// parent's, so the parent must have been laid out first. The old implicit heap got
    /// this for free (a parent's index is always smaller); with an arena it has to be
    /// stated, which is what this method is for.
    ///
    /// Returns an owned list on purpose: callers walk it while mutating the tree.
    pub fn breadth_first(&self) -> Vec<NodeId> {
        let mut order = Vec::with_capacity(self.len());
        let mut queue = VecDeque::new();
        queue.extend(self.root);
        while let Some(id) = queue.pop_front() {
            order.push(id);
            if let Some(children) = self.children(id) {
                queue.extend(children.iter().copied());
            }
        }
        order
    }

    /// The ancestor of `node` that is a direct child of the root — the **side** of the surface
    /// `node` belongs to.
    ///
    /// The root split is what divides the surface in two, so its two children are the two halves
    /// everything else lives in; walking up to one of them answers "which side is this in?" from
    /// any depth, in one step and without reasoning about orientations. A side whose insides are
    /// split further, and split again, is still one side.
    ///
    /// `Some(node)` when `node` is already a child of the root. [`None`] for the root itself,
    /// which belongs to no side because it *is* the division, and for a tree with no root.
    pub fn top_level_ancestor(&self, node: NodeId) -> Option<NodeId> {
        let root = self.root()?;
        let mut current = node;
        loop {
            // The root has no parent, so this ends: either at a child of the root, or by
            // running out on `node` being the root itself.
            let parent = self.parent(current)?;
            if parent == root {
                return Some(current);
            }
            current = parent;
        }
    }

    /// Every node that is inside a stowed subtree, and so is not on screen at all.
    ///
    /// A split that was put away as a unit draws one bar for whatever it contains (see
    /// [`RowNode::stowed`]), so everything below it has no rectangle this frame: not a
    /// smaller one, none. The layout pass asks this once and then lays out — and forgets —
    /// accordingly.
    ///
    /// The stowed split itself is **not** in the set: it is the bar, and it very much has a
    /// rectangle. What is in the set is its subtree, however deep, including the insides of a
    /// side stowed inside another one.
    ///
    /// Built on the same parents-before-children order as [`Self::breadth_first`], which is what
    /// lets "inside a stowed side" propagate downwards in one pass.
    pub fn stowed_away(&self) -> HashSet<NodeId> {
        let mut hidden = HashSet::new();
        for id in self.breadth_first() {
            // Either this node is itself put away, or an ancestor was and this one inherited it
            // — the children are hidden the same way round.
            if (hidden.contains(&id) || self[id].is_stowed())
                && let Some(children) = self.children(id)
            {
                hidden.extend(children.iter().copied());
            }
        }
        hidden
    }

    /// Returns an iterator over all tabs in the tree.
    #[inline]
    pub fn tabs(&self) -> TabIter<'_, Tab> {
        TabIter::new(self)
    }

    /// Counts and returns the number of tabs in the whole tree.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use egui_dockyard::{DockState, Split};
    /// let mut dock_state = DockState::new(vec!["node 1", "node 2", "node 3"]);
    /// assert_eq!(dock_state.main_surface().num_tabs(), 3);
    ///
    /// let root = dock_state.main_surface().root().unwrap();
    /// let [a, _b] = dock_state.main_surface_mut().split_left(root, 0.5, vec!["tab 4", "tab 5"]);
    /// assert_eq!(dock_state.main_surface().num_tabs(), 5);
    ///
    /// dock_state.main_surface_mut().remove_leaf(a);
    /// assert_eq!(dock_state.main_surface().num_tabs(), 2);
    /// ```
    #[inline]
    pub fn num_tabs(&self) -> usize {
        self.iter().map(Node::tabs_count).sum()
    }

    /// Acquire an immutable borrow to the [`Node`] at the root of the tree.
    /// Returns [`None`] if the tree is empty.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use egui_dockyard::DockState;
    /// let mut dock_state = DockState::new(vec!["single tab"]);
    /// let root_node = dock_state.main_surface().root_node().unwrap();
    ///
    /// assert_eq!(root_node.iter_tabs().collect::<Vec<_>>(), vec![&"single tab"]);
    /// ```
    pub fn root_node(&self) -> Option<&Node<Tab>> {
        self.root.map(|root| &self[root])
    }

    /// Acquire a mutable borrow to the [`Node`] at the root of the tree.
    /// Returns [`None`] if the tree is empty.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use egui_dockyard::DockState;
    /// let mut dock_state = DockState::new(vec!["single tab"]);
    /// let root_node = dock_state.main_surface_mut().root_node_mut().unwrap();
    /// root_node.append_tab("partner tab");
    ///
    /// assert_eq!(root_node.tabs_count(), 2);
    /// ```
    pub fn root_node_mut(&mut self) -> Option<&mut Node<Tab>> {
        let root = self.root?;
        Some(&mut self[root])
    }

    /// Returns the active `Tab` inside the first leaf node, or `None` if no leaf exists
    /// in the [`Tree`].
    ///
    /// Geometry is not returned alongside: it lives in
    /// [`DockLayout`](crate::layout::DockLayout).
    #[inline]
    pub fn find_active(&mut self) -> Option<&mut Tab> {
        let first_leaf = self
            .breadth_first()
            .into_iter()
            .find(|id| self[*id].is_leaf())?;
        self.leaf_mut(first_leaf).unwrap().active_focused_mut()
    }

    /// Returns the active `Tab` inside the focused leaf node or [`None`] if it does not exist.
    #[inline]
    pub fn find_active_focused(&mut self) -> Option<&mut Tab> {
        let focused = self.focused_node?;
        self.leaf_mut(focused).ok()?.active_focused_mut()
    }

    /// Gets the id of the currently focused leaf node; returns [`None`] when no leaf is focused.
    #[inline]
    pub fn focused_leaf(&self) -> Option<NodeId> {
        self.focused_node
    }

    /// Sets the currently focused leaf to `node` if it names a leaf.
    ///
    /// Never panics: a stale id or a split simply removes focus from all nodes.
    #[inline]
    pub fn set_focused_node(&mut self, node: NodeId) {
        self.focused_node = self.leaf(node).is_ok().then_some(node);
    }

    // ------------------------------------------------------------------------
    // Structural operations
    // ------------------------------------------------------------------------

    /// Creates two new nodes by splitting a given `parent` node and assigns them as its children. The first (old) node
    /// inherits content of the `parent` from before the split, and the second (new) gets the `tabs`.
    ///
    /// `fraction` (in range 0..=1) specifies how much of the `parent` node's area the old node will attempt to occupy
    /// after the split.
    ///
    /// The new node is placed relatively to the old node, in the direction specified by `split`.
    ///
    /// Returns the ids of the old node and the new node. Note that the old node *keeps its
    /// id*: what changes is where it hangs, not who it is.
    ///
    /// # Panics
    ///
    /// If `fraction` isn't in range 0..=1.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use egui_dockyard::{DockState, SurfaceIndex, Split};
    /// let mut dock_state = DockState::new(vec!["tab 1", "tab 2"]);
    ///
    /// // At this point, the main surface only contains the leaf with tab 1 and 2.
    /// assert!(dock_state.main_surface().root_node().unwrap().is_leaf());
    ///
    /// let root = dock_state.main_surface().root().unwrap();
    /// // Split the node, giving 50% of the space to the new nodes and 50% to the old ones.
    /// let [old, new] = dock_state.main_surface_mut()
    ///     .split_tabs(root, Split::Below, 0.5, vec!["tab 3"]);
    ///
    /// assert!(dock_state.main_surface().root_node().unwrap().is_parent());
    /// assert!(dock_state.main_surface()[old].is_leaf());
    /// assert!(dock_state.main_surface()[new].is_leaf());
    /// assert_eq!(old, root, "splitting a node does not rename it");
    /// ```
    #[inline(always)]
    pub fn split_tabs(
        &mut self,
        parent: NodeId,
        split: Split,
        fraction: f32,
        tabs: Vec<Tab>,
    ) -> [NodeId; 2] {
        self.split(parent, split, fraction, Node::leaf_with(tabs))
    }

    /// Splits `parent`, placing the new node *above* the old one.
    ///
    /// Shorthand for [`split_tabs`](Self::split_tabs) with [`Split::Above`].
    #[inline(always)]
    pub fn split_above(&mut self, parent: NodeId, fraction: f32, tabs: Vec<Tab>) -> [NodeId; 2] {
        self.split(parent, Split::Above, fraction, Node::leaf_with(tabs))
    }

    /// Splits `parent`, placing the new node *below* the old one.
    ///
    /// Shorthand for [`split_tabs`](Self::split_tabs) with [`Split::Below`].
    #[inline(always)]
    pub fn split_below(&mut self, parent: NodeId, fraction: f32, tabs: Vec<Tab>) -> [NodeId; 2] {
        self.split(parent, Split::Below, fraction, Node::leaf_with(tabs))
    }

    /// Splits `parent`, placing the new node to the *left* of the old one.
    ///
    /// Shorthand for [`split_tabs`](Self::split_tabs) with [`Split::Left`].
    #[inline(always)]
    pub fn split_left(&mut self, parent: NodeId, fraction: f32, tabs: Vec<Tab>) -> [NodeId; 2] {
        self.split(parent, Split::Left, fraction, Node::leaf_with(tabs))
    }

    /// Splits `parent`, placing the new node to the *right* of the old one.
    ///
    /// Shorthand for [`split_tabs`](Self::split_tabs) with [`Split::Right`].
    #[inline(always)]
    pub fn split_right(&mut self, parent: NodeId, fraction: f32, tabs: Vec<Tab>) -> [NodeId; 2] {
        self.split(parent, Split::Right, fraction, Node::leaf_with(tabs))
    }

    /// Splits the node `target` names, putting `new` next to it in the direction given by
    /// `split`, and returns `[target, new_id]`.
    ///
    /// A fresh split node is allocated to hold the two of them; `target` keeps its id and
    /// its content, and simply gains a parent.
    ///
    /// # Panics
    ///
    /// If `fraction` isn't in range 0..=1, if `new` is not a leaf with at least one tab,
    /// if `target` is not a node of this tree, or if `target` is a leaf holding no tabs.
    pub fn split(
        &mut self,
        target: NodeId,
        split: Split,
        fraction: f32,
        new: Node<Tab>,
    ) -> [NodeId; 2] {
        assert!((0.0..=1.0).contains(&fraction));
        assert_ne!(new.tabs_count(), 0, "splitting in an empty leaf");
        assert!(self.contains(target), "no node {target} in this tree");

        // Splitting a pane that shows nothing cannot be honoured literally: one side of the
        // result would be an empty leaf — a phantom pane that renders as a blank half with a
        // separator the user can drag but never fill or close, and which `validate` rejects.
        //
        // This used to be repaired here (`new` took the empty leaf's place, and both returned
        // ids named it), because `Tree::new(vec![])` left exactly such a leaf and dropping a
        // tab onto it came through this method. That shape is gone: an empty dock is a tree
        // with no root, and a tab dropped onto it arrives as `TabDestination::EmptySurface`.
        // So no empty leaf can reach here through the crate any more, and the repair became a
        // second statement of the rule — kept instead as the contract, loudly.
        assert!(
            !self[target].get_leaf().is_some_and(LeafNode::is_empty),
            "split of {target}, which is a leaf holding no tabs; \
             an empty leaf is not a state this tree admits (see `Tree::new`)"
        );

        let grandparent = self.parent(target);
        let where_target_sat = grandparent.map(|split_id| {
            self[split_id]
                .get_row()
                .expect("a parent is always a split")
                .index_of(target)
                .expect("a child is known to its parent")
        });

        let new_id = self.nodes.insert(NodeEntry {
            parent: None,
            node: new,
        });

        let horizontal = matches!(split, Split::Left | Split::Right);
        // Whether the newcomer lands on the near side of `target` — left of it, or above it.
        let before = matches!(split, Split::Left | Split::Above);

        // **The row `target` already sits in, when it lays out along the same axis.** Splitting a
        // pane of a row of two used to wrap a fresh row around it and leave a row inside a row;
        // the picture was the same and the *hand* was not, because a fraction is a share of its
        // own rectangle and the outer boundary then dragged the inner one along with it. Joining
        // the row instead divides only `target`'s own weight, so every other boundary of the row
        // stays exactly where the user left it (`RowNode::insert_beside`).
        //
        // The other orientation still allocates: a column inside a row is what the user asked
        // for, not an accident of spelling.
        if let (Some(parent), Some(index)) = (grandparent, where_target_sat)
            && self[parent]
                .get_row()
                .expect("a parent is always a row")
                .is_horizontal()
                == horizontal
        {
            self[parent]
                .get_row_mut()
                .expect("a parent is always a row")
                .insert_beside(index, new_id, fraction, before);
            self.nodes.get_mut(new_id).unwrap().parent = Some(parent);
            self.focused_node = Some(new_id);
            self.node_update_collapsed(new_id);
            return [target, new_id];
        }

        let children = match before {
            // The new node takes the first slot, pushing the old one to the second.
            true => [new_id, target],
            false => [target, new_id],
        };
        // The new row starts with empty collapsing bookkeeping; `node_update_collapsed`
        // below settles it, and the whole chain of ancestors with it, once the children are
        // linked. Inheriting it from `target` would be writing down a value that is about to
        // be overwritten anyway — and the wrong one, since the row now holds `new` too.
        let split_node = Node::Row(RowNode::pair(horizontal, children, fraction));
        let split_id = self.nodes.insert(NodeEntry {
            parent: grandparent,
            node: split_node,
        });

        self.nodes.get_mut(target).unwrap().parent = Some(split_id);
        self.nodes.get_mut(new_id).unwrap().parent = Some(split_id);

        match (grandparent, where_target_sat) {
            (Some(grandparent), Some(index)) => self[grandparent]
                .get_row_mut()
                .expect("a parent is always a split")
                .set_child(index, split_id),
            _ => self.root = Some(split_id),
        }

        self.focused_node = Some(new_id);
        self.node_update_collapsed(new_id);

        [target, new_id]
    }

    /// Removes the given leaf from the [`Tree`].
    ///
    /// Its sibling takes the place of their common parent, keeping its own id and its
    /// whole subtree. Removing the root leaf empties the tree.
    ///
    /// # Panics
    ///
    /// If `node` is not a live leaf of this tree.
    pub fn remove_leaf(&mut self, node: NodeId) {
        assert!(
            self.node(node).is_ok_and(Node::is_leaf),
            "remove_leaf on {node}, which is not a live leaf"
        );

        let Some(parent) = self.parent(node) else {
            // Removing the root leaf empties the tree; focus must go with it, and so must
            // the collapsed height — a tree with no leaves has no collapsed rows.
            self.nodes.clear();
            self.root = None;
            self.focused_node = None;
            self.set_collapsed(false);
            self.set_collapsed_leaf_count(0);
            return;
        };

        // Read out of the row before anything is written to it: the index, how many children it
        // has, and — for the dissolving case below — the one that would be left.
        let (index, child_count, sibling) = {
            let row = self[parent].get_row().expect("a parent is always a row");
            let index = row.index_of(node).expect("a child is known to its parent");
            let sibling = row.children()[if index.0 == 0 { 1 } else { 0 }];
            (index, row.children().len(), sibling)
        };

        // **A row of three loses a child and stays a row.** Only a row down to its last child
        // stops being one, which is the case below; that used to be every removal, because
        // every row held two. The weight goes back to the row rather than to a neighbour —
        // decision 5 of the n-ary plan — so the survivors keep their ratios to each other.
        if child_count > 2 {
            self[parent]
                .get_row_mut()
                .expect("a parent is always a row")
                .remove_child(index);
            self.nodes.remove(node);

            // The neighbour that moved into the gap, or the one before it at the far end.
            let children = self.children(parent).expect("a row keeps its children");
            let neighbour = children[index.0.min(children.len() - 1)];

            if self.focused_node == Some(node) {
                // The nearest surviving leaf, spelled over a row the way `first_leaf(sibling)`
                // spells it over a pair.
                self.focused_node = self.first_leaf(neighbour);
            }

            // From a *child*, because `node_update_collapsed` settles the ancestors of what it
            // is given and the row itself has just lost a leaf: handed the row, it would start
            // one level too high and leave the row's own count describing the child that went.
            self.node_update_collapsed(neighbour);
            return;
        }

        // Down to one child: the row is not a row any more, and the survivor takes its place.
        let grandparent = self.parent(parent);
        let where_parent_sat = grandparent.map(|split_id| {
            self[split_id]
                .get_row()
                .expect("a parent is always a split")
                .index_of(parent)
                .expect("a child is known to its parent")
        });

        // Promote the sibling into the parent's place.
        self.nodes.get_mut(sibling).unwrap().parent = grandparent;
        match (grandparent, where_parent_sat) {
            (Some(grandparent), Some(index)) => self[grandparent]
                .get_row_mut()
                .expect("a parent is always a split")
                .set_child(index, sibling),
            _ => self.root = Some(sibling),
        }

        self.nodes.remove(node);
        self.nodes.remove(parent);

        if self.focused_node == Some(node) {
            // Focus moves to the nearest surviving leaf, which is the closest one inside
            // the promoted sibling. Nothing else can have moved, so this is the whole
            // repair — the heap version had to re-point focus at every level it shifted.
            self.focused_node = self.first_leaf(sibling);
        }

        // The chain above the promoted sibling has one leaf fewer than it counted, and the
        // dropped one may have been the last uncollapsed leaf under some ancestor. Both
        // are read every frame as a height (`collapsed_leaf_count * tab_bar.height`), so a
        // stale number here is a layout that quietly drifts, not a panic.
        self.node_update_collapsed(sibling);
    }

    /// The first leaf inside the subtree rooted at `top` (including `top` itself).
    fn first_leaf(&self, top: NodeId) -> Option<NodeId> {
        let mut queue = VecDeque::from([top]);
        while let Some(id) = queue.pop_front() {
            match self.children(id) {
                None => return Some(id),
                Some(children) => queue.extend(children.iter().copied()),
            }
        }
        None
    }

    /// Pushes a tab to the first `Leaf` it finds, or creates a root leaf if the tree is empty.
    pub fn push_to_first_leaf(&mut self, tab: Tab) {
        match self.root.and_then(|root| self.first_leaf(root)) {
            Some(leaf) => {
                // Go through `append_tab` rather than inlining the push: it is the one
                // place that keeps the focus history in sync with the auto-focus this
                // method performs.
                self.leaf_mut(leaf).unwrap().append_tab(tab);
                self.focused_node = Some(leaf);
            }
            None => {
                let root = self.nodes.insert(NodeEntry {
                    parent: None,
                    node: Node::leaf(tab),
                });
                self.root = Some(root);
                self.focused_node = Some(root);
            }
        }
    }

    /// Pushes `tab` to the currently focused leaf.
    ///
    /// If no leaf is focused it will be pushed to the first available leaf.
    ///
    /// If no leaf is available then a new leaf will be created.
    pub fn push_to_focused_leaf(&mut self, tab: Tab) {
        match self.focused_node {
            Some(node) if self.leaf(node).is_ok() => {
                self.leaf_mut(node).unwrap().append_tab(tab);
            }
            _ => self.push_to_first_leaf(tab),
        }
    }

    /// Sets which is the active tab within a specific node.
    ///
    /// # Errors
    /// If the node is stale, not a leaf, or if the tab index is out of bounds.
    #[inline]
    pub fn set_active_tab(&mut self, node: NodeId, tab_index: impl Into<TabIndex>) -> Result {
        self.leaf_mut(node)?.set_active_tab(tab_index.into())
    }

    /// Removes the tab at the given ([`NodeId`], [`TabIndex`]) pair.
    ///
    /// If the node is emptied after the tab is removed, the node will also be removed.
    ///
    /// Returns the removed tab if it exists, or `None` otherwise.
    pub fn remove_tab(&mut self, (node, tab_index): (NodeId, TabIndex)) -> Option<Tab> {
        self.remove_tab_choosing((node, tab_index), None)
    }

    /// Removes a tab, with `successor` naming who takes the focus.
    ///
    /// See [`LeafNode::remove_tab_choosing`] for what `successor` means and when it is used.
    #[track_caller]
    pub fn remove_tab_choosing(
        &mut self,
        (node, tab_index): (NodeId, TabIndex),
        successor: Option<TabId>,
    ) -> Option<Tab> {
        let leaf = self.leaf_mut(node).ok()?;
        let tab = leaf.remove_tab_choosing(tab_index, successor);
        if leaf.is_empty() {
            self.remove_leaf(node);
        }
        tab
    }

    // ------------------------------------------------------------------------
    // Bulk edits
    // ------------------------------------------------------------------------

    /// Returns a new [`Tree`] while mapping and filtering the tab type.
    ///
    /// Leaves that lose all their tabs disappear, and a split left with one child is
    /// replaced by that child.
    pub fn filter_map_tabs<F, NewTab>(&self, mut function: F) -> Tree<NewTab>
    where
        F: FnMut(&Tab) -> Option<NewTab>,
    {
        let mut new_tree = Tree {
            nodes: Arena::default(),
            root: None,
            focused_node: None,
            // Settled by `recompute_collapsed` below, once the shape is known: copying
            // these across would describe the tree we started from, not the one built.
            collapsed: false,
            collapsed_leaf_count: 0,
        };
        new_tree.root = self.root.and_then(|root| {
            let mut focus = None;
            let copied = self.copy_filtered(root, &mut new_tree, &mut function, &mut focus);
            new_tree.focused_node = focus;
            copied
        });
        new_tree.recompute_collapsed();
        new_tree
    }

    /// Copies the subtree at `id` into `target`, dropping tabs the `function` rejects.
    ///
    /// Returns the id the subtree got in `target`, or `None` if nothing of it survived.
    /// `focus` collects where the focused node of `self` ended up.
    fn copy_filtered<F, NewTab>(
        &self,
        id: NodeId,
        target: &mut Tree<NewTab>,
        function: &mut F,
        focus: &mut Option<NodeId>,
    ) -> Option<NodeId>
    where
        F: FnMut(&Tab) -> Option<NewTab>,
    {
        let copied = match &self[id] {
            Node::Leaf(leaf) => {
                let leaf = leaf.filter_map_tabs(&mut *function)?;
                target.nodes.insert(NodeEntry {
                    parent: None,
                    node: Node::Leaf(leaf),
                })
            }
            Node::Row(row) => {
                // Each child is copied, and a child that lost every tab drops out of the row
                // **together with its weight** — the survivors keep their ratios to each other,
                // which is the same rule `remove_leaf` follows (decision 5 of the n-ary plan).
                //
                // Only the weights are carried across: they are a decision of the user's. The
                // collapsing counts describe the subtree that *was* here, and the sweep may have
                // just dropped leaves out of it — `recompute_collapsed` in the caller settles
                // them from the shape that actually got built.
                //
                // Copied rather than re-derived through a fraction, so that a row whose weights
                // do not add up to one keeps the ones it has: the file may carry any positive
                // numbers, and a copy that renormalised them would be inventing a layout nobody
                // chose.
                let mut children = Vec::new();
                let mut shares = Vec::new();
                for (child, share) in row.children().iter().zip(row.shares()) {
                    if let Some(copied) = self.copy_filtered(*child, target, function, focus) {
                        children.push(copied);
                        shares.push(*share);
                    }
                }
                match children.len() {
                    // A row with one surviving child is not a row any more: the child takes its
                    // place, which is what the old `balance()` did by swapping slots around.
                    0 => return None,
                    1 => return Some(children[0]),
                    _ => {
                        let node = RowNode::new(row.is_horizontal(), children.clone(), shares);
                        let copied = target.nodes.insert(NodeEntry {
                            parent: None,
                            node: Node::Row(node),
                        });
                        for child in children {
                            target.nodes.get_mut(child).unwrap().parent = Some(copied);
                        }
                        copied
                    }
                }
            }
        };
        if self.focused_node == Some(id) {
            *focus = Some(copied);
        }
        Some(copied)
    }

    /// Returns a new [`Tree`] while mapping the tab type.
    pub fn map_tabs<F, NewTab>(&self, mut function: F) -> Tree<NewTab>
    where
        F: FnMut(&Tab) -> NewTab,
    {
        self.filter_map_tabs(move |tab| Some(function(tab)))
    }

    /// Returns a new [`Tree`] while filtering the tab type.
    /// Leaves that lose all their tabs are removed.
    pub fn filter_tabs<F>(&self, mut predicate: F) -> Tree<Tab>
    where
        F: FnMut(&Tab) -> bool,
        Tab: Clone,
    {
        self.filter_map_tabs(move |tab| predicate(tab).then(|| tab.clone()))
    }

    /// Removes all tabs for which `predicate` returns `false`.
    /// Leaves that lose all their tabs are removed as well.
    pub fn retain_tabs<F>(&mut self, mut predicate: F)
    where
        F: FnMut(&mut Tab) -> bool,
    {
        let leaves: Vec<NodeId> = self
            .iter_indexed()
            .filter(|(_, node)| node.is_leaf())
            .map(|(id, _)| id)
            .collect();
        let mut emptied = Vec::new();
        for id in leaves {
            let leaf = self.leaf_mut(id).unwrap();
            leaf.retain_tabs(&mut predicate);
            if leaf.is_empty() {
                emptied.push(id);
            }
        }
        for id in emptied {
            // Removing one leaf can remove its parent, but never another leaf, so every
            // id collected above is still live unless the tree was emptied entirely.
            if self.contains(id) {
                self.remove_leaf(id);
            }
        }
    }

    // ------------------------------------------------------------------------
    // Collapsing
    // ------------------------------------------------------------------------

    /// Folds or opens a leaf, and settles everything that follows from it.
    ///
    /// This is the only entry point into folding, and it is one call on purpose.
    /// [`LeafNode::fold`] is the single decision in the whole scheme — the user makes
    /// it; every number a split or the tree itself stores is *derived* from the leaves
    /// below. Setting the field without recomputing the ancestors leaves the tree describing
    /// a shape it no longer has, and nothing in the type system asks for the second step:
    /// that exact omission was found and fixed twice (see `FINDINGS.md`, the collapsed-rows
    /// entries) before it became a single operation here.
    ///
    /// `fold` carries the *axis* as well as the yes/no — see [`Fold`]. Which axis is a choice
    /// the gesture makes and the tree keeps; whether it can be honoured is the layout's call,
    /// since width given up under a vertical parent has nobody to take it.
    ///
    /// # Panics
    ///
    /// If `node` is not a live leaf of this tree.
    #[track_caller]
    pub fn set_leaf_fold(&mut self, node: NodeId, fold: Fold) {
        assert!(
            self[node].is_leaf(),
            "set_leaf_fold on a node that is not a leaf: folding is a decision about \
             a leaf, and every split above it is derived from its children"
        );
        self[node].set_fold(fold);
        self.node_update_collapsed(node);
    }

    /// Puts a whole split away behind one arrow, or brings it back — see
    /// [`RowNode::stowed`](crate::RowNode::stowed).
    ///
    /// Nothing inside is touched, which is the difference from collapsing each of its leaves:
    /// a subtree comes back exactly as it went away.
    ///
    /// # Panics
    ///
    /// If `node` is not a split. Stowing is a decision about a *subtree*; a leaf has no insides
    /// to keep, and asking this of one is a caller confusing it with
    /// [`Self::set_leaf_collapsed`].
    pub fn set_split_stowed(&mut self, node: NodeId, stowed: bool) {
        assert!(
            self[node].is_parent(),
            "set_split_stowed on a node that is not a split: stowing puts a subtree away, and a \
             leaf has none — use set_leaf_fold"
        );
        self[node].set_stowed(stowed);
        // The split's own bookkeeping first — its row count is now 1 (or back to its children's)
        // — and then every ancestor, which is what `node_update_collapsed` walks.
        self.update_split_collapsed(node);
        self.node_update_collapsed(node);
    }

    /// Sets the collapsing state of the [`Tree`].
    pub(crate) fn set_collapsed(&mut self, collapsed: bool) {
        self.collapsed = collapsed;
    }

    /// Returns whether the [`Tree`] is collapsed.
    pub(crate) fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    /// Sets the number of collapsed layers of leaf subnodes in the [`Tree`].
    pub(crate) fn set_collapsed_leaf_count(&mut self, collapsed_leaf_count: i32) {
        self.collapsed_leaf_count = collapsed_leaf_count;
    }

    /// Returns the number of collapsed layers of leaf subnodes in the [`Tree`].
    pub(crate) fn collapsed_leaf_count(&self) -> i32 {
        self.collapsed_leaf_count
    }

    /// Recomputes the collapsing bookkeeping of one split from its two children.
    ///
    /// Everything a split stores about collapsing is *derived*: it is collapsed exactly
    /// when both its children are, and its collapsed-leaf count is however many collapsed
    /// rows its children stack up to. The only decision in the whole scheme is
    /// [`LeafNode::collapsed`], which the user makes; every number above it follows.
    ///
    /// # Panics
    ///
    /// If `split` is not a live split of this tree.
    fn update_split_collapsed(&mut self, split: NodeId) {
        // Both numbers are read out of the children before anything is written, so that the
        // borrow of the tree ends before the two setters below need it mutably. Written as a
        // fold rather than over `left` / `right`: `max` and `sum` over two children are what
        // they always were, and over five they are what a row means.
        let (count, all_collapsed) = {
            let children = self
                .children(split)
                .expect("update_split_collapsed on a node that is not a split");
            let counts = || {
                children
                    .iter()
                    .map(|child| self[*child].collapsed_leaf_count())
            };
            // A stowed split draws one bar for the whole subtree, so it costs one row whatever
            // is inside — asked first, because the arithmetic below is about a subtree that is
            // *shown*.
            let count = if self[split].is_stowed() {
                1
            // A horizontal split stacks its children side by side, so the collapsed rows
            // overlap; a vertical one stacks them, so they add up.
            } else if self[split].is_horizontal() {
                counts().fold(0, max)
            } else {
                counts().sum()
            };
            (
                count,
                children.iter().all(|child| self[*child].is_collapsed()),
            )
        };
        self[split].set_collapsed_leaf_count(count);
        // A row has no axis of its own, so the axis half of `Fold` is not its business — see
        // `Node::set_fold`. `Bar` stands for "folded" here and reaches `fully_collapsed`.
        self[split].set_fold(if all_collapsed { Fold::Bar } else { Fold::Open });
    }

    /// Mirrors the root's bookkeeping onto the tree itself, which is where the window
    /// surface reads the collapsed height from.
    fn sync_collapsed_from_root(&mut self) {
        let collapsed = self.root_node().is_some_and(Node::is_collapsed);
        let count = self.root_node().map_or(0, Node::collapsed_leaf_count);
        self.set_collapsed(collapsed);
        self.set_collapsed_leaf_count(count);
    }

    /// Updates the collapsed state of the node's ancestors, and of the tree itself.
    ///
    /// Call after anything that changes what the ancestors of `node` contain — collapsing
    /// a leaf, but also removing one: the counts describe a subtree, not the gesture that
    /// last touched it.
    pub(crate) fn node_update_collapsed(&mut self, node: NodeId) {
        let mut current = self.parent(node);
        while let Some(parent) = current {
            current = self.parent(parent);
            self.update_split_collapsed(parent);
        }
        self.sync_collapsed_from_root();
    }

    /// Recomputes the collapsing bookkeeping of every split, children before parents.
    ///
    /// For the bulk edits that rebuild the tree instead of editing it: a copied split
    /// carries the count of the subtree it *had*, so leaves the sweep dropped are still
    /// inside that number.
    fn recompute_collapsed(&mut self) {
        // `breadth_first` lists parents before children, so its reverse settles a split
        // only after both its children are final.
        for id in self.breadth_first().into_iter().rev() {
            if self[id].is_parent() {
                self.update_split_collapsed(id);
            }
        }
        self.sync_collapsed_from_root();
    }

    /// A tree that is one row of `leaves`, weighted by `shares` — built by hand, for scenes.
    ///
    /// Written for stage 6, when nothing in the crate built a row of three and the layout of one
    /// had to be judged anyway; its docket then said "to go when `split` can build the same row".
    /// [`split`](Self::split) now can — and this stays, because what it is really for is naming
    /// the **weights** outright. A scene that wants `1 : 3` gets it in one call instead of a
    /// split at a fraction chosen so that the renderer happens to land there, and the lesson of
    /// stages 4 and 5 was exactly that equal weights hide the path weights take.
    ///
    /// Returns the tree, the row's id, and the leaves in order. The first leaf is focused.
    #[cfg(test)]
    pub(crate) fn row_by_hand(
        horizontal: bool,
        leaves: Vec<Vec<Tab>>,
        shares: Vec<Share>,
    ) -> (Self, NodeId, Vec<NodeId>) {
        assert!(leaves.len() >= 2, "a row holds at least two children");
        assert_eq!(leaves.len(), shares.len(), "one weight per leaf");
        let mut tree = Self::default();
        let children: Vec<NodeId> = leaves
            .into_iter()
            .map(|tabs| {
                tree.nodes.insert(NodeEntry {
                    parent: None,
                    node: Node::leaf_with(tabs),
                })
            })
            .collect();
        let row = tree.nodes.insert(NodeEntry {
            parent: None,
            node: Node::Row(RowNode::new(horizontal, children.clone(), shares)),
        });
        for &child in &children {
            tree.nodes.get_mut(child).unwrap().parent = Some(row);
        }
        tree.root = Some(row);
        tree.focused_node = Some(children[0]);
        tree.recompute_collapsed();
        (tree, row, children)
    }

    /// Puts a row of fresh leaves where `leaf` is, keeping `leaf`'s place in its parent — built
    /// by hand, for scenes. Returns the new row and its leaves.
    ///
    /// The one shape [`split`](Self::split) cannot make: a row **inside a row of the same
    /// orientation**. Splitting the same way twice joins the row it is in, which is the whole of
    /// stage 7 — so from stage 7 on, a nested same-axis row is a state the tree still admits
    /// (loading keeps a *stowed* one, and a regrouping can leave one behind) but no gesture
    /// produces. A property that only such a shape can show — `collapsed_strip_height` over a
    /// subtree of several collapsed rows, found missing by mutation at stage 6 — would otherwise
    /// have no scene at all.
    ///
    /// The weights are equal: a scene that cares about weights says so through
    /// [`row_by_hand`](Self::row_by_hand).
    #[cfg(test)]
    pub(crate) fn nest_row_by_hand(
        &mut self,
        leaf: NodeId,
        horizontal: bool,
        leaves: Vec<Vec<Tab>>,
    ) -> (NodeId, Vec<NodeId>) {
        assert!(leaves.len() >= 2, "a row holds at least two children");
        assert!(self[leaf].is_leaf(), "the node replaced must be a leaf");
        let where_it_sat = self.parent(leaf).map(|parent| {
            self[parent]
                .get_row()
                .expect("a parent is always a row")
                .index_of(leaf)
                .expect("a child is known to its parent")
        });
        let parent = self.parent(leaf);
        let children: Vec<NodeId> = leaves
            .into_iter()
            .map(|tabs| {
                self.nodes.insert(NodeEntry {
                    parent: None,
                    node: Node::leaf_with(tabs),
                })
            })
            .collect();
        let shares = vec![Share(1.0); children.len()];
        let row = self.nodes.insert(NodeEntry {
            parent,
            node: Node::Row(RowNode::new(horizontal, children.clone(), shares)),
        });
        for &child in &children {
            self.nodes.get_mut(child).unwrap().parent = Some(row);
        }
        match (parent, where_it_sat) {
            (Some(parent), Some(index)) => self[parent]
                .get_row_mut()
                .expect("a parent is always a row")
                .set_child(index, row),
            _ => self.root = Some(row),
        }
        if self.focused_node == Some(leaf) {
            self.focused_node = Some(children[0]);
        }
        self.nodes.remove(leaf);
        self.recompute_collapsed();
        (row, children)
    }

    // ------------------------------------------------------------------------
    // Lookups
    // ------------------------------------------------------------------------

    /// Find a given tab based on `predicate`.
    ///
    /// Returns which node and where in that node the tab is; the [`NodeId`] always names a
    /// leaf. In case there are several hits, only the first is returned.
    pub fn find_tab_from(&self, predicate: impl Fn(&Tab) -> bool) -> Option<(NodeId, TabIndex)> {
        for node_id in self.breadth_first() {
            let Ok(leaf) = self.leaf(node_id) else {
                continue;
            };
            if let Some((tab_index, _)) = leaf.iter_tabs_indexed().find(|(_, tab)| predicate(tab)) {
                return Some((node_id, tab_index));
            }
        }
        None
    }
}

impl<Tab> Tree<Tab>
where
    Tab: PartialEq,
{
    /// Find the given tab.
    ///
    /// Returns in which node and where in that node the tab is; the [`NodeId`] always
    /// names a leaf. In case there are several hits, only the first is returned.
    pub fn find_tab(&self, needle_tab: &Tab) -> Option<(NodeId, TabIndex)> {
        self.find_tab_from(|tab| tab == needle_tab)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[derive(Copy, Clone, Debug, PartialEq)]
    struct Tab(u64);

    /// An empty dock has no node to split, so the empty vector builds no root.
    ///
    /// The `tree_ops` fuzz target found the predecessor of this: `Tree::new(vec![])` left an
    /// empty root leaf, and splitting it made that leaf the *child* of a fresh split — a blank
    /// pane the user could neither fill nor close, which the oracle rejects. The repair used
    /// to live inside `split`; now the state it repaired cannot be built.
    #[test]
    fn an_empty_tree_has_nothing_to_split() {
        let tree: Tree<Tab> = Tree::new(vec![]);

        assert_eq!(tree.root(), None, "no root, so no id to hand to `split`");
        assert_eq!(tree.len(), 0);
        assert_eq!(tree.validate(), Ok(()));
    }

    /// **Splitting a panel of a row joins that row, and the boundary lands where it was asked
    /// for** — measured from the panel that was split, not from the row.
    ///
    /// Two claims in one scene, and each is a way the join can be wrong. That the row grows
    /// rather than nesting is the feature; that the newcomer takes `1 − fraction` of the
    /// *target's own room*, on the side the `Split` names, is what makes the picture the same as
    /// the nested spelling drew. Swapping the two sides survived the whole suite when this was
    /// written: the acceptance oracle of the plan reads boundaries off the screen and asserts
    /// only that they are *local*, so a row of equal thirds is equal thirds either way round.
    #[test]
    fn splitting_a_panel_of_a_row_joins_it_at_the_fraction_asked_for() {
        // A quarter and not a half, because at a half the two sides of the split are the same
        // number and swapping them is invisible: the mutant this test is for reverses which of
        // the two keeps `fraction`.
        for (split, expected) in [
            // `b` joins to the right of `a`, so `a` keeps the first quarter of its own half.
            (Split::Right, [0.125, 0.375, 0.5]),
            // ...and to the left of it, so `a` is the one pushed off — and it is *still* the
            // first of the two that keeps a quarter, because `fraction` names the near side.
            (Split::Left, [0.125, 0.375, 0.5]),
        ] {
            let mut tree = Tree::new(vec![Tab(0)]);
            let root = tree.root().unwrap();
            let [a, c] = tree.split_right(root, 0.5, vec![Tab(1)]);
            let row = tree.root().unwrap();
            let [_, b] = tree.split(a, split, 0.25, Node::leaf(Tab(2)));

            assert_eq!(
                tree.parent(b),
                Some(row),
                "{split:?}: the newcomer joined the row instead of nesting a second one"
            );
            let children = tree.children(row).unwrap().to_vec();
            assert_eq!(children.len(), 3, "{split:?}: one row of three");
            // Which of the two is first depends on the side; the *weights* do not.
            let order = match split {
                Split::Left => vec![b, a, c],
                _ => vec![a, b, c],
            };
            assert_eq!(children, order, "{split:?}: in screen order");
            let shares: Vec<f32> = tree[row]
                .get_row()
                .unwrap()
                .shares()
                .iter()
                .map(|share| share.0)
                .collect();
            assert_eq!(shares, expected, "{split:?}: the room `a` had, halved");
            assert_eq!(tree.validate(), Ok(()));
        }
    }

    /// **A row of three loses a panel and stays a row, and the survivors keep their ratios.**
    ///
    /// Decision 5 of the n-ary plan, taken with Стас: the weight goes back to the *row*, not to a
    /// neighbour. Every boundary moves and no proportion changes — which is the only answer that
    /// treats the row as a row rather than as the pair it used to be, where "the sibling takes
    /// the place of their common parent" was the whole of removal.
    ///
    /// The weights are deliberately unequal. With three equal children, dropping the middle one
    /// and dropping the *last* weight give the same two numbers, and a mutant that removes the
    /// wrong one survives — which is exactly what it did before this test existed.
    #[test]
    fn a_row_of_three_that_loses_a_panel_keeps_the_others_in_proportion() {
        let mut tree = Tree::new(vec![Tab(0)]);
        let root = tree.root().unwrap();
        // `a` a quarter, then the remaining three quarters split two-to-one: [¼, ½, ¼].
        let [a, c] = tree.split_right(root, 0.25, vec![Tab(1)]);
        let row = tree.root().unwrap();
        let [_, d] = tree.split(c, Split::Right, 2.0 / 3.0, Node::leaf(Tab(2)));
        assert_eq!(tree.children(row).unwrap(), [a, c, d]);

        tree.remove_leaf(c);

        assert_eq!(tree.root(), Some(row), "the row did not dissolve");
        assert_eq!(
            tree.children(row).unwrap(),
            [a, d],
            "the middle one is gone"
        );
        let at = tree[row].get_row().unwrap().boundary(GapIndex(0));
        assert!(
            (at - 0.5).abs() < 1e-6,
            "the two survivors weighed a quarter each, so the boundary is the middle: {at}"
        );
        assert_eq!(tree.validate(), Ok(()));

        // And down to two, removal dissolves the row as it always did.
        tree.remove_leaf(a);
        assert_eq!(
            tree.root(),
            Some(d),
            "the last one standing takes the place"
        );
        assert_eq!(tree.validate(), Ok(()));
    }

    /// And if an empty leaf is fabricated anyway, `split` says so instead of quietly repairing
    /// it: the arena is reachable from inside the crate, so the contract needs a witness.
    #[test]
    #[should_panic(expected = "which is a leaf holding no tabs")]
    fn splitting_a_fabricated_empty_leaf_panics() {
        let mut tree: Tree<Tab> = Tree::default();
        let root = tree.nodes.insert(NodeEntry {
            parent: None,
            node: Node::leaf_with(Vec::new()),
        });
        tree.root = Some(root);

        tree.split(root, Split::Right, 0.5, Node::leaf_with(vec![Tab(1)]));
    }

    /// Checks that `retain` works after removing a node
    #[test]
    fn remove_and_retain() {
        let mut tree: Tree<Tab> = Tree::new(vec![]);
        tree.push_to_focused_leaf(Tab(0));
        let (n0, _t0) = tree.find_tab(&Tab(0)).unwrap();
        tree.split_below(n0, 0.5, vec![Tab(1)]);

        let i1 = tree.find_tab(&Tab(1)).unwrap();
        tree.remove_tab(i1);
        assert_eq!(tree.len(), 1, "the split collapsed back into a single leaf");

        tree.retain_tabs(|_| true);
        assert!(tree.find_tab(&Tab(0)).is_some());
    }

    /// The identity claim, stated as a test: a structural edit anywhere else in the tree
    /// leaves an id alone. The heap representation renamed nodes on exactly these edits.
    #[test]
    fn ids_survive_structural_edits() {
        let mut tree = Tree::new(vec![Tab(0)]);
        let root = tree.root().unwrap();

        let [old, right] = tree.split_right(root, 0.5, vec![Tab(1)]);
        assert_eq!(old, root, "the split node keeps its id");

        // Split again deeper down, then remove that leaf again.
        let [_, deep] = tree.split_below(right, 0.5, vec![Tab(2)]);
        assert_eq!(
            tree.find_tab(&Tab(0)),
            Some((root, TabIndex(0))),
            "an unrelated leaf is untouched by a split elsewhere"
        );

        tree.remove_leaf(deep);
        assert_eq!(
            tree.find_tab(&Tab(0)),
            Some((root, TabIndex(0))),
            "...and by a removal elsewhere"
        );
        assert_eq!(tree.find_tab(&Tab(1)), Some((right, TabIndex(0))));
    }

    /// A removed node's id must not resolve afterwards — neither to nothing, nor (worse)
    /// to whichever node took its place.
    #[test]
    fn a_removed_id_stops_resolving() {
        let mut tree = Tree::new(vec![Tab(0)]);
        let root = tree.root().unwrap();
        let [_, right] = tree.split_right(root, 0.5, vec![Tab(1)]);

        tree.remove_leaf(right);
        assert!(!tree.contains(right));
        assert!(tree.node(right).is_err());
        assert_eq!(tree.root(), Some(root), "the sibling is promoted to root");
    }

    #[test]
    fn breadth_first_lists_parents_before_children() {
        let mut tree = Tree::new(vec![Tab(0)]);
        let root = tree.root().unwrap();
        let [left, right] = tree.split_right(root, 0.5, vec![Tab(1)]);
        let [_, deep] = tree.split_below(right, 0.5, vec![Tab(2)]);

        let order = tree.breadth_first();
        assert_eq!(order.len(), tree.len());
        let position = |id: NodeId| order.iter().position(|other| *other == id).unwrap();
        for id in &order {
            if let Some(parent) = tree.parent(*id) {
                assert!(
                    position(parent) < position(*id),
                    "parent {parent} came after its child {id}"
                );
            }
        }
        assert!(position(left) > position(tree.root().unwrap()));
        assert!(position(deep) > position(right));
    }

    // ------------------------------------------------------------------------
    // Collapsing bookkeeping
    //
    // None of this is a structural invariant, so `validate` cannot see it: the counts are
    // a *height* the renderer reads every frame (`collapsed_leaf_count * tab_bar.height`).
    // A stale one is a layout that drifts silently, which is why it needs its own tests.
    // ------------------------------------------------------------------------

    /// Collapses a leaf the way the tab bar's button does.
    fn collapse(tree: &mut Tree<Tab>, leaf: NodeId) {
        tree.set_leaf_fold(leaf, Fold::Bar);
    }

    /// A stack of three leaves, the lower two collapsed: `V(top, V(mid, low))`.
    fn stack_with_two_collapsed() -> (Tree<Tab>, [NodeId; 3]) {
        let mut tree = Tree::new(vec![Tab(0)]);
        let root = tree.root().unwrap();
        let [top, mid] = tree.split_below(root, 0.5, vec![Tab(1)]);
        let [mid, low] = tree.split_below(mid, 0.5, vec![Tab(2)]);
        collapse(&mut tree, mid);
        collapse(&mut tree, low);
        (tree, [top, mid, low])
    }

    /// Removing a collapsed leaf must take its row out of every ancestor's count.
    ///
    /// Inherited from the heap version and kept through the arena refactor on purpose, so
    /// that the refactor stayed a parity change; this is the behavioural fix.
    #[test]
    fn removing_a_collapsed_leaf_updates_ancestor_counts() {
        let (mut tree, [_, mid, low]) = stack_with_two_collapsed();
        assert_eq!(
            tree.collapsed_leaf_count(),
            2,
            "two collapsed rows to start"
        );

        tree.remove_leaf(low);

        assert_eq!(
            tree[tree.parent(mid).unwrap()].collapsed_leaf_count(),
            1,
            "the removed row is still counted by the ancestor"
        );
        assert_eq!(tree.collapsed_leaf_count(), 1);
        assert_eq!(tree.validate(), Ok(()));
    }

    /// Emptying the tree takes the collapsed height with it — a tree with no leaves has
    /// no collapsed rows to reserve space for.
    #[test]
    fn removing_the_last_leaf_clears_the_collapsed_height() {
        let mut tree = Tree::new(vec![Tab(0)]);
        let root = tree.root().unwrap();
        collapse(&mut tree, root);
        assert_eq!(tree.collapsed_leaf_count(), 1);

        tree.remove_leaf(root);

        assert!(tree.is_empty());
        assert_eq!(tree.collapsed_leaf_count(), 0);
        assert!(!tree.is_collapsed());
    }

    /// The tree-level count must follow the root even while the root itself is *not*
    /// collapsed — a partially collapsed dock is the normal case, not the corner one.
    #[test]
    fn a_partially_collapsed_tree_still_reports_its_rows() {
        let mut tree = Tree::new(vec![Tab(0)]);
        let root = tree.root().unwrap();
        let [top, bottom] = tree.split_below(root, 0.5, vec![Tab(1)]);

        collapse(&mut tree, bottom);

        assert!(!tree[tree.parent(top).unwrap()].is_collapsed());
        assert_eq!(
            tree.collapsed_leaf_count(),
            1,
            "one collapsed leaf, one collapsed row"
        );
    }

    /// Collapsing is one operation, not a pair a caller has to remember to complete: the
    /// ancestors must already agree by the time `set_leaf_collapsed` returns.
    #[test]
    fn collapsing_a_leaf_settles_its_ancestors_in_one_call() {
        let mut tree = Tree::new(vec![Tab(0)]);
        let root = tree.root().unwrap();
        let [top, bottom] = tree.split_below(root, 0.5, vec![Tab(1)]);
        let split = tree.parent(top).unwrap();

        tree.set_leaf_fold(bottom, Fold::Bar);
        assert_eq!(tree[split].collapsed_leaf_count(), 1);
        assert_eq!(tree.collapsed_leaf_count(), 1, "the tree mirrors its root");

        tree.set_leaf_fold(top, Fold::Bar);
        assert!(
            tree[split].is_collapsed(),
            "both children collapsed makes the split collapsed"
        );

        // Expanding walks the same path back down.
        tree.set_leaf_fold(bottom, Fold::Open);
        assert!(!tree[split].is_collapsed());
        assert_eq!(tree[split].collapsed_leaf_count(), 1);
    }

    /// A split's collapsing is derived, so there is nothing to *set* on it — the argument
    /// names a decision only a leaf can hold.
    #[test]
    #[should_panic(expected = "not a leaf")]
    fn a_split_cannot_be_collapsed_directly() {
        let mut tree = Tree::new(vec![Tab(0)]);
        let root = tree.root().unwrap();
        let [top, _] = tree.split_below(root, 0.5, vec![Tab(1)]);
        let split = tree.parent(top).unwrap();

        tree.set_leaf_fold(split, Fold::Bar);
    }

    /// The copying sweeps rebuild the tree, so a split they copy carries the count of the
    /// subtree it *had* — including the leaves the sweep just dropped.
    #[test]
    fn a_copying_sweep_recounts_the_rows_it_dropped() {
        let (tree, [_, _, low]) = stack_with_two_collapsed();
        let dropped = tree[low].iter_tabs().next().copied().unwrap();

        let filtered = tree.filter_tabs(|tab| *tab != dropped);

        assert_eq!(filtered.collapsed_leaf_count(), 1);
        let root = filtered.root().unwrap();
        assert_eq!(filtered[root].collapsed_leaf_count(), 1);
        assert_eq!(filtered.validate(), Ok(()));
    }

    /// **What a copying sweep may not throw away.** The counts above are *derived* and the sweep
    /// is right to recompute them; the weights are not — they are where the user last left the
    /// boundaries, and a copy that recentred them would silently rearrange a dock that was only
    /// asked to drop a tab.
    ///
    /// `copy_filtered` has always said so in a comment beside the line, and nothing checked it:
    /// a mutant writing `0.5` there passed the whole suite (293 tests) during stage 4's mutant
    /// round. Found because the line changed — from carrying one `fraction` to carrying the
    /// weights — and the class is one this crate keeps paying for: a property stated in prose
    /// next to the code that implements it, with no oracle anywhere.
    #[test]
    fn a_copying_sweep_keeps_the_boundaries_the_user_left() {
        let mut tree = Tree::new(vec![1, 2]);
        let root = tree.root().unwrap();
        let [left, _] = tree.split_right(root, 0.25, vec![3]);
        tree.split_below(left, 0.8, vec![4]);

        // Every tab survives, so the filtered tree is the same dock — and has to look like it.
        let filtered = tree.filter_tabs(|_| true);

        let boundaries = |tree: &Tree<i32>| {
            tree.breadth_first()
                .into_iter()
                .filter_map(|id| tree[id].get_row().map(RowNode::fraction))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            boundaries(&filtered),
            boundaries(&tree),
            "the copy moved a boundary nobody asked it to move"
        );
        assert_eq!(
            boundaries(&tree),
            vec![0.25, 0.8],
            "the scene has two of them"
        );
    }
}
