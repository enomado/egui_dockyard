use crate::TabIndex;

mod leaf;
mod split;
pub use leaf::{LeafNode, TabId};
pub use split::SplitNode;

/// Represents an abstract node of a [`Tree`](crate::Tree).
///
/// There is no `Empty` variant: a node either exists in the tree's arena or it does not.
/// The old implicit-heap layout needed one to describe holes in the `Vec`, and those holes
/// were a source of their own bugs — a "removed" subtree that still owned its tabs, a
/// split with one live child, a serialized layout mostly made of `Empty`.
#[derive(Clone, Debug)]
pub enum Node<Tab> {
    /// Contains the actual tabs.
    Leaf(LeafNode<Tab>),

    /// Parent node in the vertical orientation: first child on top, second below.
    Vertical(SplitNode),

    /// Parent node in the horizontal orientation: first child on the left, second on the
    /// right.
    Horizontal(SplitNode),
}

impl<Tab> Node<Tab> {
    /// Constructs a leaf node with a given `tab`.
    #[inline(always)]
    pub fn leaf(tab: Tab) -> Self {
        Self::Leaf(LeafNode::new(vec![tab]))
    }

    /// Constructs a leaf node with a given list of `tabs`.
    #[inline(always)]
    pub fn leaf_with(tabs: Vec<Tab>) -> Self {
        Self::Leaf(LeafNode::new(tabs))
    }

    /// Get immutable access to the leaf data of this node, if it contains any (i.e. is a leaf).
    pub fn get_leaf(&self) -> Option<&LeafNode<Tab>> {
        match self {
            Node::Leaf(leaf_node) => Some(leaf_node),
            _ => None,
        }
    }

    /// Get mutable access to the leaf data of this node, if it contains any (i.e. is a leaf).
    pub fn get_leaf_mut(&mut self) -> Option<&mut LeafNode<Tab>> {
        match self {
            Node::Leaf(leaf_node) => Some(leaf_node),
            _ => None,
        }
    }

    /// Get immutable access to the split data of this node, if it is a split.
    pub fn get_split(&self) -> Option<&SplitNode> {
        match self {
            Node::Vertical(split) | Node::Horizontal(split) => Some(split),
            Node::Leaf(_) => None,
        }
    }

    /// Get mutable access to the split data of this node, if it is a split.
    pub fn get_split_mut(&mut self) -> Option<&mut SplitNode> {
        match self {
            Node::Vertical(split) | Node::Horizontal(split) => Some(split),
            Node::Leaf(_) => None,
        }
    }

    /// Returns `true` if the node is a [`Leaf`](Node::Leaf), otherwise `false`.
    #[inline(always)]
    pub const fn is_leaf(&self) -> bool {
        matches!(self, Self::Leaf { .. })
    }

    /// Returns `true` if the node is a [`Horizontal`](Node::Horizontal), otherwise `false`.
    #[inline(always)]
    pub const fn is_horizontal(&self) -> bool {
        matches!(self, Self::Horizontal { .. })
    }

    /// Returns `true` if the node is a [`Vertical`](Node::Vertical), otherwise `false`.
    #[inline(always)]
    pub const fn is_vertical(&self) -> bool {
        matches!(self, Self::Vertical { .. })
    }

    /// Returns `true` if the node is either [`Horizontal`](Node::Horizontal) or [`Vertical`](Node::Vertical),
    /// otherwise `false`.
    #[inline(always)]
    pub const fn is_parent(&self) -> bool {
        self.is_horizontal() || self.is_vertical()
    }

    /// Returns `true` if the node is collapsed, otherwise `false`.
    #[inline(always)]
    pub fn is_collapsed(&self) -> bool {
        match self {
            Node::Leaf(leaf) => leaf.collapsed,
            Node::Horizontal(split) | Node::Vertical(split) => split.fully_collapsed,
        }
    }

    /// Returns the number of layers of collapsed leaf subnodes.
    pub fn collapsed_leaf_count(&self) -> i32 {
        match self {
            Node::Horizontal(split) | Node::Vertical(split) => split.collapsed_leaf_count,
            Node::Leaf(leaf) => i32::from(leaf.collapsed),
        }
    }

    /// Provides an immutable iterator over the tabs inside this node.
    ///
    /// The iterator is empty if the node is not a [`Leaf`](Node::Leaf).
    #[inline]
    pub fn iter_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.get_leaf().into_iter().flat_map(LeafNode::iter_tabs)
    }

    /// Returns an [`Iterator`] of tabs in this node with their corresponding [`TabIndex`].
    pub fn iter_tabs_indexed(&self) -> impl Iterator<Item = (TabIndex, &Tab)> {
        self.iter_tabs()
            .enumerate()
            .map(|(index, tab)| (TabIndex(index), tab))
    }

    /// Returns a mutable [`Iterator`] of tabs in this node.
    ///
    /// The iterator is empty if the node is not a [`Leaf`](Node::Leaf).
    #[inline]
    pub fn iter_tabs_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.get_leaf_mut()
            .into_iter()
            .flat_map(LeafNode::iter_tabs_mut)
    }

    /// Returns a mutable [`Iterator`] of tabs in this node with their corresponding [`TabIndex`].
    pub fn iter_tabs_mut_indexed(&mut self) -> impl Iterator<Item = (TabIndex, &mut Tab)> {
        self.iter_tabs_mut()
            .enumerate()
            .map(|(index, tab)| (TabIndex(index), tab))
    }

    /// Adds `tab` to the node and sets it as the active tab.
    ///
    /// # Panics
    ///
    /// If `self` is not a [`Leaf`](Node::Leaf) node.
    #[track_caller]
    #[inline]
    pub fn append_tab(&mut self, tab: Tab) {
        match self {
            Node::Leaf(leaf) => leaf.append_tab(tab),
            _ => panic!("node was not a leaf"),
        }
    }

    /// Sets the collapsing state of the node.
    #[inline]
    pub fn set_collapsed(&mut self, collapsed: bool) {
        match self {
            Node::Leaf(leaf) => leaf.collapsed = collapsed,
            Node::Vertical(split) | Node::Horizontal(split) => split.fully_collapsed = collapsed,
        }
    }

    /// Sets the number of layers of collapsed leaf subnodes.
    ///
    /// # Panics
    ///
    /// Panics if `self` is neither a [`Vertical`](Node::Vertical) nor a [`Horizontal`](Node::Horizontal) node.
    #[track_caller]
    #[inline]
    pub fn set_collapsed_leaf_count(&mut self, count: i32) {
        match self {
            Node::Horizontal(split) | Node::Vertical(split) => split.collapsed_leaf_count = count,
            _ => panic!("node was neither vertical nor horizontal"),
        }
    }

    /// Adds a `tab` to the node.
    ///
    /// # Panics
    ///
    /// Panics if `self` is not a leaf, or `index > tabs_count()`.
    #[track_caller]
    #[inline]
    pub fn insert_tab(&mut self, index: TabIndex, tab: Tab) {
        match self {
            Node::Leaf(leaf) => leaf.insert_tab(index, tab),
            _ => panic!("node was not a leaf!"),
        }
    }

    /// Removes a tab at given `index` from the node.
    ///
    /// Returns the removed tab, or `None` if the node is not a leaf or has no such tab.
    #[inline]
    pub fn remove_tab(&mut self, tab_index: TabIndex) -> Option<Tab> {
        match self {
            Node::Leaf(leaf) => leaf.remove_tab(tab_index),
            _ => None,
        }
    }

    /// Gets the number of tabs in the node.
    #[inline]
    pub fn tabs_count(&self) -> usize {
        match self {
            Node::Leaf(leaf) => leaf.len(),
            _ => 0,
        }
    }
}
