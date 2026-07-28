use crate::core::WindowState;
use crate::core::tree::{Node, NodeId, TabIndex, Tree};

/// A borrowed view of one surface of a [`DockState`](crate::DockState).
///
/// This is a *view*, not a stored value: the main surface and the windows live in separate
/// fields of the dock, so there is no `Surface` object anywhere to hand out a reference to.
/// Iterating surfaces therefore yields this by value, and the borrows it carries are the
/// borrows of whatever it names.
#[derive(Debug, Clone, Copy)]
pub enum SurfaceRef<'a, Tab> {
    /// A window slot that no window occupies — a hole left by a closed window, kept so the
    /// windows after it keep their [`WindowIndex`](crate::WindowIndex).
    Empty,

    /// The main surface.
    Main(&'a Tree<Tab>),

    /// A floating window: its tree, and where the window sits.
    Window(&'a Tree<Tab>, &'a WindowState),
}

/// A mutably borrowed view of one surface. See [`SurfaceRef`].
#[derive(Debug)]
pub enum SurfaceMut<'a, Tab> {
    /// An unoccupied window slot.
    Empty,

    /// The main surface.
    Main(&'a mut Tree<Tab>),

    /// A floating window: its tree, and where the window sits.
    Window(&'a mut Tree<Tab>, &'a mut WindowState),
}

impl<'a, Tab> SurfaceRef<'a, Tab> {
    /// Is this an unoccupied window slot?
    ///
    /// Note this is about the *slot*, not about content: a surface holding a tree with no
    /// tabs is a legitimate empty dock and answers `false`.
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// The tree of this surface, or `None` if the slot is unoccupied.
    ///
    /// The borrow returned is the one this view carries, not one of the view itself, so it
    /// outlives the (usually temporary) `SurfaceRef`.
    pub const fn node_tree(&self) -> Option<&'a Tree<Tab>> {
        match *self {
            Self::Empty => None,
            Self::Main(tree) | Self::Window(tree, _) => Some(tree),
        }
    }

    /// The window state of this surface, or `None` if it is not a window.
    pub const fn window_state(&self) -> Option<&'a WindowState> {
        match *self {
            Self::Empty | Self::Main(_) => None,
            Self::Window(_, state) => Some(state),
        }
    }

    /// Returns an [`Iterator`] of nodes in this surface's tree.
    ///
    /// If the slot is unoccupied, the returned [`Iterator`] is empty.
    ///
    /// Takes `self` by value (the view is [`Copy`]) so that the iterator borrows the tree and
    /// not the — usually temporary — view standing in front of it.
    pub fn iter_nodes(self) -> impl Iterator<Item = &'a Node<Tab>> {
        self.node_tree().into_iter().flat_map(Tree::iter)
    }

    /// Returns an [`Iterator`] of nodes in this surface's tree with their [`NodeId`].
    pub fn iter_nodes_indexed(self) -> impl Iterator<Item = (NodeId, &'a Node<Tab>)> {
        self.node_tree().into_iter().flat_map(Tree::iter_indexed)
    }

    /// Returns an [`Iterator`] of **all** tabs in this surface's tree and their paths
    /// within the surface.
    pub fn iter_all_tabs(self) -> impl Iterator<Item = ((NodeId, TabIndex), &'a Tab)> {
        self.iter_nodes_indexed().flat_map(|(node_index, node)| {
            node.iter_tabs_indexed()
                .map(move |(tab_index, tab)| ((node_index, tab_index), tab))
        })
    }
}

impl<'a, Tab> SurfaceMut<'a, Tab> {
    /// Is this an unoccupied window slot? See [`SurfaceRef::is_empty`].
    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// The tree of this surface, consuming the view so the borrow it carries survives.
    pub fn into_node_tree_mut(self) -> Option<&'a mut Tree<Tab>> {
        match self {
            Self::Empty => None,
            Self::Main(tree) | Self::Window(tree, _) => Some(tree),
        }
    }

    /// The tree of this surface, or `None` if the slot is unoccupied.
    pub fn node_tree(&self) -> Option<&Tree<Tab>> {
        match self {
            Self::Empty => None,
            Self::Main(tree) | Self::Window(tree, _) => Some(tree),
        }
    }

    /// Mutable access to the tree of this surface, or `None` if the slot is unoccupied.
    pub fn node_tree_mut(&mut self) -> Option<&mut Tree<Tab>> {
        match self {
            Self::Empty => None,
            Self::Main(tree) | Self::Window(tree, _) => Some(tree),
        }
    }

    /// Mutable access to the window state, or `None` if this is not a window.
    pub fn window_state_mut(&mut self) -> Option<&mut WindowState> {
        match self {
            Self::Empty | Self::Main(_) => None,
            Self::Window(_, state) => Some(state),
        }
    }

    /// Returns a mutable [`Iterator`] of nodes in this surface's tree with their [`NodeId`].
    ///
    /// Consumes the view: the borrows it hands out are the ones it carries, and a `&mut`
    /// cannot be handed out twice.
    pub fn into_iter_nodes_mut_indexed(self) -> impl Iterator<Item = (NodeId, &'a mut Node<Tab>)> {
        self.into_node_tree_mut()
            .into_iter()
            .flat_map(Tree::iter_mut_indexed)
    }

    /// Returns a mutable [`Iterator`] of **all** tabs in this surface's tree and their paths
    /// within the surface. Consumes the view, as [`into_iter_nodes_mut_indexed`](Self::into_iter_nodes_mut_indexed) does.
    pub fn into_iter_all_tabs_mut(self) -> impl Iterator<Item = ((NodeId, TabIndex), &'a mut Tab)> {
        self.into_iter_nodes_mut_indexed()
            .flat_map(|(node_index, node)| {
                node.iter_tabs_mut_indexed()
                    .map(move |(tab_index, tab)| ((node_index, tab_index), tab))
            })
    }
}
