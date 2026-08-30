use crate::core::tree::TabIndex;

mod leaf;
mod row;
pub use leaf::{LeafNode, TabId};
pub use row::{RowNode, Share};

/// Represents an abstract node of a [`Tree`](crate::Tree).
///
/// There is no `Empty` variant: a node either exists in the tree's arena or it does not.
/// The old implicit-heap layout needed one to describe holes in the `Vec`, and those holes
/// were a source of their own bugs — a "removed" subtree that still owned its tabs, a
/// row with one live child, a serialized layout mostly made of `Empty`.
///
/// There is no `Vertical` / `Horizontal` pair either, for a reason that took longer to see:
/// they carried identical data and were matched *together* in fourteen places, which is a
/// field spelled as a variant. The axis now lives in [`RowNode::is_horizontal`], and the
/// question every reader was actually asking — [`is_horizontal`](Self::is_horizontal) /
/// [`is_vertical`](Self::is_vertical) — is unchanged.
#[derive(Clone, Debug)]
pub enum Node<Tab> {
    /// Contains the actual tabs.
    Leaf(LeafNode<Tab>),

    /// Parent node: children laid out along one axis, which [`RowNode`] names.
    Row(RowNode),
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

    /// Get immutable access to the row data of this node, if it is a row.
    pub fn get_row(&self) -> Option<&RowNode> {
        match self {
            Node::Row(row) => Some(row),
            Node::Leaf(_) => None,
        }
    }

    /// Get mutable access to the row data of this node, if it is a row.
    pub fn get_row_mut(&mut self) -> Option<&mut RowNode> {
        match self {
            Node::Row(row) => Some(row),
            Node::Leaf(_) => None,
        }
    }

    /// Returns `true` if the node is a [`Leaf`](Node::Leaf), otherwise `false`.
    #[inline(always)]
    pub const fn is_leaf(&self) -> bool {
        matches!(self, Self::Leaf { .. })
    }

    /// Returns `true` if the node is a row laying its children out side by side.
    ///
    /// Kept as a question about the *node*, although the axis now lives one level down: every
    /// reader in the crate asked it here when it was a variant, and a refactor whose whole
    /// claim is parity is not the place to move them. A leaf answers `false` to both this and
    /// [`is_vertical`](Self::is_vertical), exactly as it did when neither variant matched it.
    #[inline(always)]
    pub const fn is_horizontal(&self) -> bool {
        matches!(self, Self::Row(row) if row.is_horizontal())
    }

    /// Returns `true` if the node is a row stacking its children.
    #[inline(always)]
    pub const fn is_vertical(&self) -> bool {
        matches!(self, Self::Row(row) if row.is_vertical())
    }

    /// Returns `true` if the node is a [`Row`](Node::Row), otherwise `false`.
    #[inline(always)]
    pub const fn is_parent(&self) -> bool {
        matches!(self, Self::Row(_))
    }

    /// Returns `true` if the node is collapsed, otherwise `false`.
    #[inline(always)]
    /// Whether this node shows a bar instead of its contents — which is the question every
    /// caller here is really asking, and why a stowed row answers yes as readily as one whose
    /// leaves were collapsed one by one. How it got that way is [`RowNode::stowed`]'s
    /// business, and matters only when bringing it back.
    pub fn is_collapsed(&self) -> bool {
        match self {
            Node::Leaf(leaf) => leaf.collapsed,
            Node::Row(row) => row.stowed || row.fully_collapsed,
        }
    }

    /// Whether this node is a row that was put away as a unit — see [`RowNode::stowed`].
    ///
    /// Always `false` for a leaf: a leaf has nothing inside to keep, so collapsing it is the
    /// whole of what can happen to it.
    pub fn is_stowed(&self) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Row(row) => row.stowed,
        }
    }

    /// Puts this row away as a unit, or brings it back. No-op on a leaf.
    pub(crate) fn set_stowed(&mut self, stowed: bool) {
        match self {
            Node::Leaf(_) => {}
            Node::Row(row) => row.stowed = stowed,
        }
    }

    /// Returns the number of layers of collapsed leaf subnodes.
    pub fn collapsed_leaf_count(&self) -> i32 {
        match self {
            Node::Row(row) => row.collapsed_leaf_count,
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
    ///
    /// Deliberately not public: on its own it makes the tree inconsistent, because every
    /// row above the node derives its bookkeeping from what it contains. The operation
    /// callers want is [`Tree::set_leaf_collapsed`](crate::Tree::set_leaf_collapsed).
    #[inline]
    pub(crate) fn set_collapsed(&mut self, collapsed: bool) {
        match self {
            Node::Leaf(leaf) => leaf.collapsed = collapsed,
            Node::Row(row) => row.fully_collapsed = collapsed,
        }
    }

    /// Sets the number of layers of collapsed leaf subnodes.
    ///
    /// Deliberately not public: this number is derived from the node's children, so the
    /// only correct writer is the tree's own recomputation.
    ///
    /// # Panics
    ///
    /// Panics if `self` is not a [`Row`](Node::Row).
    #[track_caller]
    #[inline]
    pub(crate) fn set_collapsed_leaf_count(&mut self, count: i32) {
        match self {
            Node::Row(row) => row.collapsed_leaf_count = count,
            Node::Leaf(_) => panic!("node was not a row"),
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
