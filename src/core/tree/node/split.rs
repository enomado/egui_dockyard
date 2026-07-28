///the inner data of a [``Node::Horizontal``](crate::Node)/[``Node::Vertical``](crate::Node), which splits into two further nodes.
///
/// Carries no geometry: the rectangle a split occupies is derived by the layout pass
/// every frame and lives in [`DockLayout`](crate::layout::DockLayout), keyed by
/// `(surface, node)`. `fraction` — *where* the split sits inside whatever rectangle it
/// is given — is genuine state and stays here.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct SplitNode {
    /// The fraction taken by the top child of this node.
    pub fraction: f32,

    /// Whether all subnodes are collapsed.
    pub fully_collapsed: bool,

    /// The number of collapsed leaf subnodes.
    pub collapsed_leaf_count: i32,
}

impl SplitNode {
    /// Create a new ``SplitNode``
    pub const fn new(fraction: f32, fully_collapsed: bool, collapsed_leaf_count: i32) -> Self {
        Self {
            fraction,
            fully_collapsed,
            collapsed_leaf_count,
        }
    }
}
