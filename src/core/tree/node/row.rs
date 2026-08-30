use crate::core::tree::{ChildIndex, NodeId};

/// The inner data of a [`Node::Row`](crate::Node): the children laid out along one axis, and
/// where the boundary between them sits.
///
/// Carries no geometry: the rectangle a row occupies is derived by the layout pass
/// every frame and lives in [`DockLayout`](crate::layout::DockLayout), keyed by
/// `(surface, node)`. `fraction` — *where* the boundary sits inside whatever rectangle the row
/// is given — is genuine state and stays here.
///
/// The children are named explicitly. Before the arena they were implied by position
/// (`2i + 1` / `2i + 2`) and a split could perfectly well be missing one of them — that
/// was a whole variant of [`TreeViolation`](crate::TreeViolation). Now "a row has the children
/// it says it has" holds by construction.
///
/// # Why the orientation is a field
///
/// It used to be the *variant*: `Node::Horizontal(SplitNode)` and `Node::Vertical(SplitNode)`,
/// two arms carrying identical data. Fourteen places matched them **together**
/// (`Node::Vertical(split) | Node::Horizontal(split)`), which is a field written the long way:
/// every reader that did not care about the axis still had to name both arms, and every reader
/// that did care asked `is_vertical()` anyway. The pair of arms also made "the same question,
/// once per axis" the natural shape for anything that *did* branch — and that is exactly the
/// shape the 30.08 strip bug hid in, where the horizontal branch had grown a rule the vertical
/// one had solved years earlier.
///
/// A row does not hold more than two children yet: that is stage 7 of
/// `docs/PLAN_a_row_holds_many_panels.md`, and this stage is parity.
#[derive(Clone, Debug)]
pub struct RowNode {
    /// Which axis this row lays its children out along: `true` for side by side (the first
    /// child on the left), `false` for stacked (the first child on top).
    ///
    /// Not public: the pair [`is_horizontal`](Self::is_horizontal) /
    /// [`is_vertical`](Self::is_vertical) is what every reader in the crate used to ask of the
    /// *variant*, so it is what they keep asking. A writer would be changing the axis of a row
    /// under a layout that has already cut it, and no caller has ever wanted that — a
    /// regrouping builds the row it wants (see [`Regroup`](crate::core::tree::regroup::Regroup)).
    horizontal: bool,

    /// The children, first (left / top) then second (right / bottom).
    children: [NodeId; 2],

    /// The fraction taken by the first child of this node.
    pub fraction: f32,

    /// Whether all subnodes are collapsed.
    ///
    /// Derived from the children, and **not** the same question as [`Self::stowed`]: this
    /// one is "everything inside happens to be collapsed", arrived at one leaf at a time.
    pub fully_collapsed: bool,

    /// Whether this row was put away **as a unit** — the whole subtree hidden behind one
    /// arrow, rather than each of its leaves collapsed in turn.
    ///
    /// Genuine state, and the only collapsing state a row has of its own: everything else
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
    /// One for a [`stowed`](Self::stowed) row whatever it contains: it draws a single bar,
    /// so a single row is what it costs. See `Tree::update_split_collapsed`.
    pub collapsed_leaf_count: i32,
}

impl RowNode {
    /// Creates a new [`RowNode`] over two existing nodes.
    ///
    /// The collapsing bookkeeping is *not* an argument, and deliberately so: both fields are
    /// derived from the children, so the only honest value at construction time — before
    /// the children are linked up and reachable — is the empty one. Whoever builds the row
    /// settles them afterwards through
    /// [`Tree::update_split_collapsed`](crate::Tree), directly or through one of the sweeps
    /// that call it. Taking them as arguments invited callers to pass the state of whatever
    /// used to be there, which is bookkeeping tied to a gesture rather than to a subtree —
    /// the bug class this crate has already paid for twice.
    ///
    /// `horizontal` is the first argument because it is the one thing a caller cannot derive
    /// from the others: it used to be the choice of *variant* wrapped around this value, and
    /// a constructor that took it last would read as though it were a modifier.
    pub(crate) const fn new(horizontal: bool, children: [NodeId; 2], fraction: f32) -> Self {
        Self {
            horizontal,
            children,
            fraction,
            fully_collapsed: false,
            stowed: false,
            collapsed_leaf_count: 0,
        }
    }

    /// Whether this row lays its children out side by side.
    #[inline(always)]
    pub const fn is_horizontal(&self) -> bool {
        self.horizontal
    }

    /// Whether this row stacks its children.
    #[inline(always)]
    pub const fn is_vertical(&self) -> bool {
        !self.horizontal
    }

    /// This row's children, in order: first (left / top), then second (right / bottom).
    ///
    /// A slice and not a pair, although a row holds exactly two of them today. Almost every
    /// reader of this method walks a subtree, counts leaves or forwards the children to a
    /// queue — questions a row of five answers exactly as a pair does, and which therefore
    /// need not be written twice when a row can hold five. The readers that genuinely need
    /// *two* say so by name, through [`children_pair`](Self::children_pair).
    #[inline(always)]
    pub fn children(&self) -> &[NodeId] {
        &self.children
    }

    /// Both children, first (left / top) then second (right / bottom).
    ///
    /// The pair spelling, deliberately a *different name* rather than a destructuring of
    /// [`children`](Self::children): every place that still needs a row to hold exactly two
    /// is then a grep for one identifier instead of a reading of the crate. Each caller carries
    /// a note saying why a pair is the honest shape there, or which stage owes it a row.
    #[inline(always)]
    pub const fn children_pair(&self) -> [NodeId; 2] {
        self.children
    }

    /// The child at the given position, or `None` if this row has no child there.
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

    /// Where `child` sits among this row's children, or `None` if it is not one of them.
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
    /// If this row has no child at that position. Every caller inside the crate holds an
    /// index [`index_of`](Self::index_of) just handed it about *this* row, so an
    /// out-of-range one is a bug in the caller rather than a case to answer — unlike
    /// [`child`](Self::child), which also serves a route read from disk.
    #[inline(always)]
    pub(crate) fn set_child(&mut self, index: ChildIndex, child: NodeId) {
        self.children[index.0] = child;
    }
}
