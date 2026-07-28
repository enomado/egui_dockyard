use crate::core::tree::{NodeId, Side};

/// The inner data of a [`Node::Horizontal`](crate::Node) / [`Node::Vertical`](crate::Node),
/// which splits into two further nodes.
///
/// Carries no geometry: the rectangle a split occupies is derived by the layout pass
/// every frame and lives in [`DockLayout`](crate::layout::DockLayout), keyed by
/// `(surface, node)`. `fraction` — *where* the split sits inside whatever rectangle it
/// is given — is genuine state and stays here.
///
/// The two children are named explicitly. Before the arena they were implied by position
/// (`2i + 1` / `2i + 2`) and a split could perfectly well be missing one of them — that
/// was a whole variant of [`TreeViolation`](crate::TreeViolation). Now "a split has
/// exactly two children" holds by construction.
#[derive(Clone, Debug)]
pub struct SplitNode {
    /// The two children, first (left / top) then second (right / bottom).
    children: [NodeId; 2],

    /// The fraction taken by the first child of this node.
    pub fraction: f32,

    /// Whether all subnodes are collapsed.
    pub fully_collapsed: bool,

    /// The number of collapsed leaf subnodes.
    pub collapsed_leaf_count: i32,
}

impl SplitNode {
    /// Creates a new [`SplitNode`] over two existing nodes.
    ///
    /// The collapsing bookkeeping is *not* an argument, and deliberately so: both fields are
    /// derived from the two children, so the only honest value at construction time — before
    /// the children are linked up and reachable — is the empty one. Whoever builds the split
    /// settles them afterwards through
    /// [`Tree::update_split_collapsed`](crate::Tree), directly or through one of the sweeps
    /// that call it. Taking them as arguments invited callers to pass the state of whatever
    /// used to be there, which is bookkeeping tied to a gesture rather than to a subtree —
    /// the bug class this crate has already paid for twice.
    pub(crate) const fn new(children: [NodeId; 2], fraction: f32) -> Self {
        Self {
            children,
            fraction,
            fully_collapsed: false,
            collapsed_leaf_count: 0,
        }
    }

    /// Both children, first (left / top) then second (right / bottom).
    #[inline(always)]
    pub const fn children(&self) -> [NodeId; 2] {
        self.children
    }

    /// The child on the given side.
    #[inline(always)]
    pub const fn child(&self, side: Side) -> NodeId {
        self.children[side.as_index()]
    }

    /// Which side `child` is on, or `None` if it is not a child of this split.
    #[inline]
    pub fn side_of(&self, child: NodeId) -> Option<Side> {
        match child {
            _ if self.children[0] == child => Some(Side::Left),
            _ if self.children[1] == child => Some(Side::Right),
            _ => None,
        }
    }

    /// Points the given side at another node. Used when the tree re-links a subtree.
    #[inline(always)]
    pub(crate) fn set_child(&mut self, side: Side, child: NodeId) {
        self.children[side.as_index()] = child;
    }
}
