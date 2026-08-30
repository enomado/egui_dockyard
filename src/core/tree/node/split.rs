use crate::core::tree::{ChildIndex, NodeId};

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
    ///
    /// Derived from the two children, and **not** the same question as [`Self::stowed`]: this
    /// one is "everything inside happens to be collapsed", arrived at one leaf at a time.
    pub fully_collapsed: bool,

    /// Whether this split was put away **as a unit** — the whole subtree hidden behind one
    /// arrow, rather than each of its leaves collapsed in turn.
    ///
    /// Genuine state, and the only collapsing state a split has of its own: everything else
    /// here is derived from the children. That is the point of it. Putting a side away could
    /// have been expressed as "collapse all of its leaves", which needs no new field — but then
    /// bringing it back has nothing to bring back *to*, and a leaf the user had collapsed inside
    /// it days ago would return expanded. A subtree that is stowed keeps its insides exactly as
    /// they were, for the same reason a hidden half keeps its `fraction`.
    ///
    /// Serialized (`#[serde(default)]`), so layouts written before this existed load as "not
    /// stowed", which is what they were.
    pub stowed: bool,

    /// The number of collapsed leaf subnodes.
    ///
    /// One for a [`stowed`](Self::stowed) split whatever it contains: it draws a single bar,
    /// so a single row is what it costs. See `Tree::update_split_collapsed`.
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
            stowed: false,
            collapsed_leaf_count: 0,
        }
    }

    /// Both children, first (left / top) then second (right / bottom).
    #[inline(always)]
    pub const fn children(&self) -> [NodeId; 2] {
        self.children
    }

    /// The child at the given position, or `None` if this split has no child there.
    ///
    /// `Option` rather than a panic, because one caller reads the position out of a **file**:
    /// the focus route of a saved layout is a sequence of these (see
    /// [`persist`](crate::core::tree::persist)), and nothing stops a file from naming a fifth
    /// child of a pair. `Side` could not express that; an index can, so the out-of-range case
    /// became reachable the moment it did.
    #[inline]
    pub fn child(&self, index: ChildIndex) -> Option<NodeId> {
        self.children.get(index.0).copied()
    }

    /// Where `child` sits among this split's children, or `None` if it is not one of them.
    #[inline]
    pub fn index_of(&self, child: NodeId) -> Option<ChildIndex> {
        self.children
            .iter()
            .position(|&candidate| candidate == child)
            .map(ChildIndex)
    }

    /// Points the given position at another node. Used when the tree re-links a subtree.
    ///
    /// # Panics
    ///
    /// If this split has no child at that position. Every caller inside the crate holds an
    /// index [`index_of`](Self::index_of) just handed it about *this* split, so an
    /// out-of-range one is a bug in the caller rather than a case to answer — unlike
    /// [`child`](Self::child), which also serves a route read from disk.
    #[inline(always)]
    pub(crate) fn set_child(&mut self, index: ChildIndex, child: NodeId) {
        self.children[index.0] = child;
    }
}
