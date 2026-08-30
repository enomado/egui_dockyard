use crate::core::tree::{ChildIndex, NodeId};

/// How much of a row's length one of its children asks for, **relative to its siblings**.
///
/// Positive (zero is allowed and means "no length at all"), finite, and deliberately **not
/// normalised**: the row divides by the sum when it lays out, so `[Share(1.0), Share(1.0)]` and
/// `[Share(0.5), Share(0.5)]` are the same picture. That is what keeps edits total — inserting a
/// child is a `push`, removing one is a `remove`, and no other child's number changes. A
/// normalised vector would have to be rewritten whole on every edit, which is a second place for
/// rounding to accumulate.
///
/// # Why weights and not boundaries
///
/// A row could as well have stored where its boundaries sit (`0 ≤ b₀ ≤ b₁ ≤ … ≤ 1`). Weights win
/// twice, and both reasons are about what the type makes *impossible*:
///
/// * the invariant is **local** — "this weight is finite and not negative", one number at a time.
///   Boundaries carry a global one, monotonicity, which makes "a boundary overtook its
///   neighbour" an expressible state that then has to be defended against at every writer;
/// * a weight is where growth goes: a minimum size, a fixed-size child, a child that does not
///   grow — the `flex-grow` shape every mature layout engine converges on. A boundary has
///   nowhere to put any of that.
///
/// Pixel extents were rejected separately: they would put screen state back into the model,
/// which is exactly what this crate spent a refactor taking out (`rect` / `viewport` off the
/// nodes and into [`DockLayout`](crate::layout::DockLayout)).
///
/// A newtype from the first line rather than a bare `f32` to be tidied later, because a weight,
/// a fraction of a parent, a pixel extent and a boundary in `0..1` are four different things in
/// this crate and three of them are `f32`.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Share(pub f32);

/// The inner data of a [`Node::Row`](crate::Node): the children laid out along one axis, and how
/// much of the row's length each of them takes.
///
/// Carries no geometry: the rectangle a row occupies is derived by the layout pass
/// every frame and lives in [`DockLayout`](crate::layout::DockLayout), keyed by
/// `(surface, node)`. The [`shares`](Self::shares) — *how the row's length is divided*, whatever
/// rectangle it is given — are genuine state and stay here.
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

    /// The children, in order along the axis: leftmost / topmost first.
    ///
    /// A `Vec` although a row holds exactly two of them today — the type is n-ary and the
    /// content is not, which is the whole of stage 4 in
    /// `docs/PLAN_a_row_holds_many_panels.md`. Every reader that walks them was already written
    /// against a slice (stage 2); what is left to change is the four places that *build* a row,
    /// and they are stage 7's.
    children: Vec<NodeId>,

    /// One weight per child, in [`children`](Self::children) order.
    ///
    /// Kept the same length as `children` by construction — the only writer of either is
    /// [`new`](Self::new), which checks it, and [`set_fraction`](Self::set_fraction), which
    /// replaces both weights of a pair at once. Nothing in the crate can grow one and not the
    /// other yet; the operations that could are stage 7's, and they will take the two together.
    shares: Vec<Share>,

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
    ///
    /// # Panics
    ///
    /// If `children` and `shares` are of different lengths, or if the row would be empty. That
    /// the two vectors agree is the one invariant of this type a *reader* cannot check for
    /// itself, and the constructor is the only place in the crate where they are set — so it is
    /// stated here rather than left to [`validate`](crate::Tree::validate), which would be
    /// judging a state nothing can build.
    pub(crate) fn new(horizontal: bool, children: Vec<NodeId>, shares: Vec<Share>) -> Self {
        assert_eq!(
            children.len(),
            shares.len(),
            "a row has one weight per child"
        );
        assert!(!children.is_empty(), "a row with no children is not a row");
        Self {
            horizontal,
            children,
            shares,
            fully_collapsed: false,
            stowed: false,
            collapsed_leaf_count: 0,
        }
    }

    /// A row of exactly two children, whose boundary sits at `fraction` of its length.
    ///
    /// The pair spelling, named apart from [`new`](Self::new) for the same reason
    /// [`children_pair`](Self::children_pair) is named apart from [`children`](Self::children):
    /// every place that still *builds* a row of two is then a grep for one identifier instead of
    /// a reading of the crate. All four of them are owed a row by stage 7 of
    /// `docs/PLAN_a_row_holds_many_panels.md` — splitting a node, regrouping a subtree, copying
    /// a filtered tree, and loading a file, which is where the binary shape enters from disk.
    pub(crate) fn pair(horizontal: bool, children: [NodeId; 2], fraction: f32) -> Self {
        Self::new(
            horizontal,
            children.to_vec(),
            vec![Share(fraction), Share(1.0 - fraction)],
        )
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
    ///
    /// # Panics
    ///
    /// If this row does not hold exactly two children. Unreachable while the tree is binary by
    /// content, and deliberately loud rather than silently answering about the first two: a
    /// caller here is one that has *not* been taught rows yet, and the moment stage 7 builds a
    /// row of three, this is the list of places that have to be visited.
    #[inline]
    #[track_caller]
    pub fn children_pair(&self) -> [NodeId; 2] {
        match self.children[..] {
            [first, second] => [first, second],
            ref children => panic!(
                "a pair was asked of a row of {}; see `RowNode::children`",
                children.len()
            ),
        }
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

    /// This row's weights, one per child, in [`children`](Self::children) order.
    ///
    /// Relative to each other and not to anything else — see [`Share`]. A reader that wants a
    /// proportion divides by [`total_share`](Self::total_share).
    #[inline(always)]
    pub fn shares(&self) -> &[Share] {
        &self.shares
    }

    /// What this row's weights add up to. Always finite and greater than zero for a row that
    /// passes [`validate`](crate::Tree::validate), which is what makes dividing by it total.
    #[inline]
    pub fn total_share(&self) -> f32 {
        self.shares.iter().map(|share| share.0).sum()
    }

    /// The proportion of this row taken by its first child.
    ///
    /// The pair spelling of "where the boundary sits", and the one every reader of the layout
    /// still asks: while a row holds two children it has exactly one boundary, and this is it.
    /// A row of three has two, which is why they get names of their own in stage 5 of
    /// `docs/PLAN_a_row_holds_many_panels.md` (`GapIndex` there); until then a caller of this
    /// is a caller that reads a row as a pair.
    ///
    /// # Parity
    ///
    /// Exactly the number [`pair`](Self::pair) was built from, for every `fraction` in `0..=1`,
    /// and not approximately: `f + fl(1 − f)` rounds to exactly `1.0` in `f32` there — the error
    /// of `fl(1 − f)` is at most half an ulp of a value below one, which is at most half the ulp
    /// just below `1.0` — so the division below is by exactly one. That is what lets this stage
    /// claim the pixels did not move, rather than claim they moved by less than anyone can see.
    ///
    /// Pinned by `the_boundary_a_pair_was_built_from_comes_back_exactly`, because an argument
    /// about floating point in a comment is a claim and not a check.
    #[inline]
    pub fn fraction(&self) -> f32 {
        self.shares[0].0 / self.total_share()
    }

    /// Moves the boundary of a two-child row to `fraction` of its length.
    ///
    /// Writes *both* weights, because that is what "the boundary is here" means when the row is
    /// a pair: `[fraction, 1 − fraction]`. A caller passing a `fraction` outside `0..=1` gets a
    /// negative weight, which [`validate`](crate::Tree::validate) rejects — the old global rule
    /// "the fraction is within the interval it measures" is exactly the local rule "no weight is
    /// negative", seen from the other side.
    ///
    /// # Panics
    ///
    /// If this row does not hold exactly two children — see
    /// [`children_pair`](Self::children_pair), which this shares its fate with. Over a row of
    /// three, "the fraction" names nothing.
    #[inline]
    #[track_caller]
    pub fn set_fraction(&mut self, fraction: f32) {
        assert_eq!(
            self.shares.len(),
            2,
            "a fraction was written to a row of {}; a row of three has no single boundary",
            self.shares.len()
        );
        self.shares = vec![Share(fraction), Share(1.0 - fraction)];
    }
}

#[cfg(test)]
mod tests {
    use super::{RowNode, Share};
    use crate::core::tree::NodeId;

    /// Two ids, which is all a row needs to be built directly: nothing here walks the tree.
    fn row(shares: Vec<Share>) -> RowNode {
        let children = (0..shares.len())
            .map(|_| NodeId::new(0, 0))
            .collect::<Vec<_>>();
        RowNode::new(true, children, shares)
    }

    /// **Weights are relative, and nothing in the crate says so yet.**
    ///
    /// Every row alive at this stage is built by [`RowNode::pair`], whose weights always add up
    /// to exactly one — so "the row divides by the sum" and "the row reads the first weight" are
    /// the same function everywhere it is called, and a mutant replacing
    /// [`total_share`](RowNode::total_share) with the constant `1.0` survives the whole suite,
    /// the corpus probes included. That is the *central* decision of this stage going unjudged:
    /// weights are deliberately **not** normalised, precisely so that stage 7 can push and remove
    /// children without rewriting everyone else's number.
    ///
    /// So it is stated here, on rows built by hand, where it is reachable. When stage 7 makes it
    /// reachable through ordinary use, this test stops being the only thing holding it.
    #[test]
    fn weights_that_do_not_add_up_to_one_still_name_a_proportion() {
        assert_eq!(row(vec![Share(3.0), Share(1.0)]).fraction(), 0.75);
        assert_eq!(row(vec![Share(1.0), Share(1.0)]).fraction(), 0.5);
        assert_eq!(row(vec![Share(0.0), Share(2.0)]).fraction(), 0.0);
        // A row of three, which no writer builds yet: "the first child's share of the row" is
        // the question that survives the shape change, and it is already answerable.
        assert_eq!(
            row(vec![Share(2.0), Share(1.0), Share(1.0)]).fraction(),
            0.5
        );
        assert_eq!(row(vec![Share(3.0), Share(1.0)]).total_share(), 4.0);
    }

    /// The parity claim of stage 4, checked rather than argued.
    ///
    /// `RowNode::fraction` is documented to answer *exactly* the number `RowNode::pair` was
    /// built from — not within an epsilon — because that is what lets this stage say the pixels
    /// did not move at all. The reasoning is that `f + fl(1 − f)` rounds to exactly `1.0` for
    /// every `f` in `0..=1`; the reasoning is in the doc, and this is the check.
    ///
    /// An epsilon comparison here would pass on an implementation that drifts, which is the one
    /// thing this test exists to catch: a drift of one ulp in a stored boundary is invisible on
    /// screen and shows up as a corpus dump that no longer diffs clean, six stages later.
    #[test]
    fn the_boundary_a_pair_was_built_from_comes_back_exactly() {
        let mut checked = 0;
        for step in 0..=10_000u32 {
            let fraction = step as f32 / 10_000.0;
            let back = RowNode::pair(true, [NodeId::new(0, 0); 2], fraction).fraction();
            assert_eq!(
                back.to_bits(),
                fraction.to_bits(),
                "a boundary at {fraction} came back as {back}"
            );
            checked += 1;
        }
        assert_eq!(checked, 10_001, "the sweep ran");
    }
}
