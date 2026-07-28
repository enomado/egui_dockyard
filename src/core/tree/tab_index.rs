use crate::core::SurfaceIndex;
use crate::core::tree::{NodeId, NodePath};

/// Position of a tab inside a [`Node`](crate::Node).
///
/// This is deliberately a *position*, not an identity — see [`TabId`](crate::TabId) for
/// the identity. A position is the right thing exactly twice: in the persisted layout
/// format, and within a single frame of UI, where the tab bar draws tabs in order and
/// hit-tests whatever the pointer is over. Holding one across a mutation is a bug.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct TabIndex(pub usize);

impl From<usize> for TabIndex {
    #[inline]
    fn from(index: usize) -> Self {
        TabIndex(index)
    }
}

/// A full path to locate a tab within an entire dock state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TabPath {
    /// Index of the surface owning the node for the tab.
    pub surface: SurfaceIndex,
    /// Identity of the node holding the tab.
    pub node: NodeId,
    /// Position of the tab in that node.
    pub tab: TabIndex,
}

impl TabPath {
    /// Creates a new fully qualified path to a tab.
    pub const fn new(surface: SurfaceIndex, node: NodeId, tab: TabIndex) -> Self {
        Self { surface, node, tab }
    }

    /// Get the node path components.
    pub const fn node_path(self) -> NodePath {
        NodePath {
            surface: self.surface,
            node: self.node,
        }
    }
}

impl From<(NodePath, TabIndex)> for TabPath {
    fn from((NodePath { surface, node }, tab): (NodePath, TabIndex)) -> Self {
        TabPath { surface, node, tab }
    }
}
