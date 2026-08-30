/// Stable identity of a node inside one [`Tree`](crate::Tree).
///
/// # Why this is not an index
///
/// The tree used to be an implicit binary heap: a node's address *was* its position
/// (`2i + 1` / `2i + 2`), so every structural edit renamed nodes. Anything that held an
/// address across such an edit — focus, a drag in flight, a geometry map, a caller that
/// split a node and then wanted to touch it again — silently addressed a *different*
/// node. Two shipped bug fixes in this repository are that class of bug.
///
/// A `NodeId` is an arena handle instead: the slot the node lives in, plus the generation
/// of that slot. Structural edits move nothing, so an id stays valid for as long as the
/// node exists, and once the node is gone the id stops resolving rather than resolving to
/// whoever took the slot over — the generation is bumped on removal.
///
/// # Contract
///
/// * An id is only meaningful for the tree that produced it. Ids from two different trees
///   (i.e. two surfaces) may compare equal while naming unrelated nodes, so always carry
///   the surface alongside — that is what [`NodePath`](crate::NodePath) is for.
/// * An id is *not* persisted. Saved layouts store the shape of the tree, and loading
///   builds a fresh arena, so ids differ between runs by design.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    slot: u32,
    generation: u32,
}

impl NodeId {
    #[inline(always)]
    pub(crate) const fn new(slot: u32, generation: u32) -> Self {
        Self { slot, generation }
    }

    /// Which arena slot this id points at.
    #[inline(always)]
    pub(crate) const fn slot(self) -> u32 {
        self.slot
    }

    /// Which occupant of that slot this id refers to.
    #[inline(always)]
    pub(crate) const fn generation(self) -> u32 {
        self.generation
    }
}

/// A full path to locate a node in an entire dock state: which surface, and which node of
/// that surface's tree.
///
/// A [`NodeId`] on its own is only meaningful inside the tree that handed it out, so
/// anything that reaches across surfaces — the drag in flight, the geometry map, the
/// focused leaf of the whole dock — carries this pair instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodePath {
    /// Index of the surface owning the node.
    pub surface: crate::core::SurfaceIndex,
    /// Identity of the node in the surface tree.
    pub node: NodeId,
}

impl NodePath {
    /// Creates a fully qualified new path to a node.
    pub const fn new(surface: crate::core::SurfaceIndex, node: NodeId) -> Self {
        Self { surface, node }
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `slot.generation` reads well in violation reports and fuzz output.
        write!(f, "{}.{}", self.slot, self.generation)
    }
}

/// Which child of a parent is meant: the child's position among its siblings.
///
/// # Why a position and not a side
///
/// This used to be `Side::{Left, Right}`, and that one type said two things at once: *which
/// of the children* and *which way round they sit on screen*. The two coincide only while a
/// parent has exactly two children — a row of three has a middle child that is on neither
/// side, and cannot be named at all in that language.
///
/// The screen-side meaning already has a type of its own ([`SideStrip`](crate::SideStrip)),
/// which is why this one could keep the other half and become an index. The convention is
/// unchanged: `0` is the first child — left of a [`Horizontal`](crate::Node::Horizontal)
/// split, top of a [`Vertical`](crate::Node::Vertical) one — so split fractions keep meaning
/// what they meant.
///
/// An index is a *position*, so it is only meaningful against the parent it was taken from,
/// and only until that parent's children change. Holding one across a structural edit is the
/// bug class [`NodeId`] exists to answer; this type is for the two places a position is the
/// honest currency — one step of a walk down the tree, and the persisted focus route, where
/// there is no identity to carry across a save.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ChildIndex(pub usize);
