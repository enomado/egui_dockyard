//! Frame-local geometry of a dock tree: which screen rectangle each node ended up in.
//!
//! # Why this is not part of the model
//!
//! Node rectangles are *derived*: they are recomputed from scratch by the layout pass on
//! every frame, out of the available area plus the split fractions. They used to be
//! stored as `rect` / `viewport` fields on the nodes themselves, which had two costs:
//!
//! * they were serialized with the tree, so every persisted layout carried the pixel
//!   coordinates of whatever window size the last session happened to use — data nobody
//!   reads back as truth;
//! * they forced `egui` into the model, blocking the core from being egui-free.
//!
//! So geometry now lives here, keyed by [`NodePath`] (`surface` + node identity), and is stored
//! in egui's temporary memory next to the [`DockArea`](crate::DockArea) id rather than in
//! the [`DockState`](crate::DockState).
//!
//! # Reading it from outside a frame
//!
//! The map is written during [`DockArea::show_inside`](crate::DockArea::show_inside) and
//! left in `ctx` afterwards, so code that runs outside the dock pass (screenshots,
//! automation, diagnostics) can ask for the last known geometry:
//!
//! ```rust
//! # use egui_dockyard::{DockLayout, DockState, NodePath, SurfaceIndex};
//! # egui::__run_test_ctx(|ctx| {
//! let dock_state = DockState::new(vec!["a tab"]);
//! let dock_id = egui::Id::new("egui_dockyard::DockArea");
//! let layout = DockLayout::load(ctx, dock_id);
//! let root = dock_state.main_surface().root().unwrap();
//! let _rect = layout.rect(NodePath::new(SurfaceIndex::main(), root));
//! # });
//! ```
//!
//! Entries survive across frames (a node keeps its last known rectangle, exactly as the
//! old fields did), but nodes that no longer exist are dropped at the end of each pass —
//! see [`DockLayout::retain_live`].

mod convert;

use std::collections::HashMap;

use egui::{Context, Id, Rect};

use crate::{DockState, GapPath, NodePath};

/// Which edge of its own split a collapsed leaf was pressed against, when the layout pass
/// shrank it sideways instead of into a row.
///
/// Left and right, and no `Top` / `Bottom`, because collapsing into a *row* is the older
/// behaviour and needs no marker: a collapsed leaf under a vertical split is a tab bar, which
/// is what a leaf draws anyway. Only the sideways case draws something else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideStrip {
    /// The strip is the left edge of its split; the sibling took the width to its right.
    Left,

    /// The strip is the right edge of its split; the sibling took the width to its left.
    Right,
}

/// Where a single node ended up on screen during the last layout pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeGeometry {
    /// The full rectangle the node occupies: for a leaf, tab bar plus tab body.
    pub rect: Rect,

    /// The tab body rectangle, i.e. `rect` minus the tab bar. Only ever set for leaves,
    /// and only for leaves that actually rendered a body this frame (a collapsed or
    /// zero-sized leaf has no viewport).
    pub viewport: Option<Rect>,

    /// Set when this leaf was collapsed *sideways* — squeezed to a narrow vertical strip
    /// against one edge of its split, with the sibling taking the width it gave up.
    ///
    /// Drawing reads this rather than deciding for itself, and that is the point: "is this
    /// leaf a strip?" must not be answered by looking at how narrow the rectangle came out.
    /// A leaf can be narrow because the user dragged the separator, and a rule phrased on
    /// the width would turn that into a strip behind their back. The layout pass knows the
    /// answer — it is the one that made the decision — so it says so here.
    pub side_strip: Option<SideStrip>,
}

/// Geometry of every node of a [`DockState`], as computed by the last layout pass — and of
/// every divider, keyed by the gap it was drawn in.
///
/// Stored in [`egui::Context`] memory under the [`DockArea`](crate::DockArea)'s id, not
/// in the dock state — it is derived data and is deliberately never serialized.
///
/// Dividers are a map of their own rather than a field of [`NodeGeometry`], because a divider
/// is not a property of a node: it is a property of a *gap* between two children of a row, and a
/// row has one fewer of those than it has children. While rows are pairs that is one per row,
/// and the field it used to be worked; a row of three has two, and a field can name only one.
/// See [`GapPath`].
#[derive(Clone, Debug, Default)]
pub struct DockLayout {
    nodes: HashMap<NodePath, NodeGeometry>,

    /// Where each divider ended up: the line drawn between two neighbouring children and,
    /// expanded a little, the one the user grabs to move the boundary. Absent for a gap the pass
    /// did **not** cut at its ratio.
    ///
    /// That second case is the whole reason this lives here rather than being re-derived by
    /// drawing. A collapsed half is given exactly the strip it needs and the divider is put
    /// *beside* it, so there is no line at the ratio: one drawn there would lie across the
    /// sibling, over space that child owns — visible, grabbable, moving nothing. Worse,
    /// grabbing a divider *writes* the ratio, which is precisely what the hidden half is
    /// keeping for when it comes back.
    ///
    /// The pass already computes this rectangle in every branch; it used to throw it away and
    /// let drawing work it out again, together with a "is there one at all?" rule that then had
    /// to be repeated everywhere and drifted the moment a branch was added. Now the branch that
    /// cuts the row is the one that says what it cut, and the answer is not optional to give:
    /// see `SplitCut` in `show/mod.rs` and [`Self::set_divider`].
    ///
    /// Note that a divider always occupies *space* — the layout leaves `separator.width`
    /// between a strip and its sibling either way. An absent entry means there is no line to
    /// paint or to hit-test, not that the two children are flush.
    dividers: HashMap<GapPath, Rect>,
}

impl DockLayout {
    /// Id under which the map is kept in egui memory, derived from the dock area's own
    /// id so that two dock areas in one context do not share geometry.
    #[inline]
    fn memory_id(dock_area_id: Id) -> Id {
        dock_area_id.with("layout")
    }

    /// Read the geometry left behind by the last pass of the dock area with this id.
    ///
    /// Returns an empty map if the dock area has not been shown yet in this context.
    pub fn load(ctx: &Context, dock_area_id: Id) -> Self {
        ctx.data_mut(|d| d.get_temp(Self::memory_id(dock_area_id)))
            .unwrap_or_default()
    }

    /// Publish this map so that later frames — and code outside the dock pass — can read
    /// it back with [`load`](Self::load).
    pub(crate) fn store(self, ctx: &Context, dock_area_id: Id) {
        ctx.data_mut(|d| d.insert_temp(Self::memory_id(dock_area_id), self));
    }

    /// Full geometry of a node, or [`None`] if it was never laid out.
    #[inline]
    pub fn get(&self, path: NodePath) -> Option<NodeGeometry> {
        self.nodes.get(&path).copied()
    }

    /// The rectangle a node occupies, or [`None`] if it was never laid out.
    #[inline]
    pub fn rect(&self, path: NodePath) -> Option<Rect> {
        self.nodes.get(&path).map(|geometry| geometry.rect)
    }

    /// The body rectangle of a leaf, or [`None`] if it has no body this frame (not a
    /// leaf, collapsed, zero-sized, or never laid out).
    #[inline]
    pub fn viewport(&self, path: NodePath) -> Option<Rect> {
        self.nodes.get(&path).and_then(|geometry| geometry.viewport)
    }

    /// Which edge this leaf was collapsed against, or [`None`] if it is not a sideways
    /// collapsed strip this frame.
    #[inline]
    pub fn side_strip(&self, path: NodePath) -> Option<SideStrip> {
        self.nodes
            .get(&path)
            .and_then(|geometry| geometry.side_strip)
    }

    /// Where the divider in `gap` was drawn this frame, or [`None`] if there is none — see
    /// [`Self::dividers`] for what "none" means and why the answer is stored rather than worked
    /// out by whoever asks. A gap that names no row (a leaf, a node never laid out, a gap past
    /// the row's last) simply has none.
    #[inline]
    pub fn divider(&self, gap: GapPath) -> Option<Rect> {
        self.dividers.get(&gap).copied()
    }

    /// Number of nodes with known geometry.
    #[inline]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether no node has known geometry (e.g. the dock area was never shown).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Record the rectangle of a node, keeping any viewport already known for it.
    ///
    /// Clears [`NodeGeometry::side_strip`], which is the whole reason that flag is safe to keep
    /// here. Entries outlive the frame that wrote them (see the module docs), so a flag that were
    /// only ever *set* would survive the leaf being expanded again and draw a strip forever.
    /// Every laid-out node gets its rectangle written on every pass, so clearing it here means
    /// the flag can only ever describe this frame: the sideways path re-asserts it immediately
    /// after, and nothing else has to remember to take it back.
    ///
    /// The row's dividers are deliberately **not** cleared here, and the asymmetry is the point
    /// rather than an oversight. `set_side_strip` is called only when there *is* a strip, so its
    /// absence has to be spelled somewhere; [`Self::set_divider`] takes an [`Option`] and is
    /// called for every gap of every laid-out row on every pass (and [`Self::forget_dividers`]
    /// for a row that is stowed), so the absence of a divider is itself an answer that arrives.
    /// The only other way an entry could go stale is the node ceasing to be a row — and a row
    /// does not become a leaf, it is *removed* and the surviving child takes its place, after
    /// which [`Self::retain_live`] drops the entry. Clearing here as well would be a guard
    /// against nothing, which is worse than none: it cannot be made to fail, so it would read as
    /// load-bearing forever.
    #[inline]
    pub(crate) fn set_rect(&mut self, path: NodePath, rect: Rect) {
        self.nodes
            .entry(path)
            .and_modify(|geometry| {
                geometry.rect = rect;
                geometry.side_strip = None;
            })
            .or_insert(NodeGeometry {
                rect,
                viewport: None,
                side_strip: None,
            });
    }

    /// Record where the divider in `gap` was cut, or that there is none.
    ///
    /// Takes an [`Option`] rather than being a "set it if there is one" call, and that is the
    /// point: every branch of the layout pass answers this question for every gap it cuts, so
    /// the answer arrives here whichever way it came out. A setter that could simply not be
    /// called would put the branch that forgets back on the map.
    #[inline]
    pub(crate) fn set_divider(&mut self, gap: GapPath, divider: Option<Rect>) {
        match divider {
            Some(rect) => {
                self.dividers.insert(gap, rect);
            }
            None => {
                self.dividers.remove(&gap);
            }
        }
    }

    /// Drop every divider of the row at `row`: it draws none this frame, whatever it holds.
    ///
    /// The answer for a row the pass does not cut at all — one stowed away as a unit, which is
    /// a single bar for its whole subtree. Said as one call over the row rather than as
    /// [`Self::set_divider`] per gap, because the branch that stows has no gaps in hand: it never
    /// looked at the children, and a row of five behind one arrow has four gaps to forget.
    #[inline]
    pub(crate) fn forget_dividers(&mut self, row: NodePath) {
        self.dividers.retain(|gap, _| gap.row != row);
    }

    /// Mark a leaf as a sideways collapsed strip against `side`.
    ///
    /// Called by the layout pass right after [`Self::set_rect`] for the same node, which is
    /// what clears any previous frame's answer.
    #[inline]
    pub(crate) fn set_side_strip(&mut self, path: NodePath, side: SideStrip) {
        if let Some(geometry) = self.nodes.get_mut(&path) {
            geometry.side_strip = Some(side);
        }
    }

    /// Drop everything known about a node, because it is not on screen this frame.
    ///
    /// Different from [`Self::retain_live`], which is about nodes that no longer *exist*: this
    /// one is for a node that exists and was deliberately not laid out — the inside of a stowed
    /// side. Both end in the same place, and they have to: an entry left behind by the last
    /// frame is not a stale rectangle nobody looks at. Everything downstream is written to ask
    /// the layout rather than work the answer out again, so a leftover entry *is* the answer.
    /// A tab body would be drawn inside the strip, and the junction handles — which flatten a
    /// chain of splits by their rectangles, several levels down from the separator being drawn
    /// — would place handles inside it from where the subtree used to be.
    ///
    /// So "not laid out" is spelled as no entry at all, which is exactly what a node that has
    /// never been shown looks like, and every reader already handles that. A row's dividers go
    /// with it, for the same reason: a line left behind by the last frame *is* the answer to
    /// "is there a divider here", and inside a stowed side there is not.
    #[inline]
    pub(crate) fn forget(&mut self, path: NodePath) {
        self.nodes.remove(&path);
        self.forget_dividers(path);
    }

    /// Record the body rectangle of a leaf.
    ///
    /// The rectangle is expected to have been recorded already by the same pass; if not
    /// (a leaf rendered without going through the layout pass), the viewport doubles as
    /// the node rectangle so the entry is never internally inconsistent.
    #[inline]
    pub(crate) fn set_viewport(&mut self, path: NodePath, viewport: Rect) {
        self.nodes
            .entry(path)
            .and_modify(|geometry| geometry.viewport = Some(viewport))
            .or_insert(NodeGeometry {
                rect: viewport,
                viewport: Some(viewport),
                side_strip: None,
            });
    }

    /// Forget the geometry of nodes that no longer exist in `dock_state`, and the dividers of
    /// rows that no longer exist.
    ///
    /// Keys are identities, so a dead entry can no longer be mistaken for a live node —
    /// this is now only about not growing forever, which is why it can be a plain
    /// "is it still there?" question.
    pub(crate) fn retain_live<Tab>(&mut self, dock_state: &DockState<Tab>) {
        let alive = |path: NodePath| {
            dock_state
                .get_surface(path.surface)
                .and_then(|surface| surface.node_tree())
                .is_some_and(|tree| tree.contains(path.node))
        };
        self.nodes.retain(|path, _| alive(*path));
        self.dividers.retain(|gap, _| alive(gap.row));
    }
}

/// The divider map's own bookkeeping, asked directly.
///
/// The screen tests say what a divider *means* (`a_hidden_half_has_no_boundary_to_drag`,
/// `a_side_can_be_stowed`); what is here is the part no scene reaches today — a map that keeps a
/// line for a row that is gone is read by nobody while every row is a pair drawn on screen, so
/// nothing would go red for it, and it would be the entry a later reader finds first.
#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect};

    use super::DockLayout;
    use crate::{DockState, GapIndex, GapPath, NodeId, NodePath, SurfaceIndex};

    fn a_rect() -> Rect {
        Rect::from_min_max(Pos2::ZERO, Pos2::new(10.0, 10.0))
    }

    fn gap(path: NodePath, index: usize) -> GapPath {
        GapPath::new(path, GapIndex(index))
    }

    /// The absence of a divider is an answer that arrives: writing `None` into a gap takes the
    /// line out, it does not leave last frame's in place.
    #[test]
    fn a_divider_set_to_none_is_gone() {
        let path = NodePath::new(SurfaceIndex::main(), NodeId::new(3, 0));
        let mut layout = DockLayout::default();
        layout.set_rect(path, a_rect());
        layout.set_divider(gap(path, 0), Some(a_rect()));
        assert!(
            layout.divider(gap(path, 0)).is_some(),
            "control: the line was recorded"
        );

        layout.set_divider(gap(path, 0), None);
        assert_eq!(layout.divider(gap(path, 0)), None);
        assert!(
            layout.rect(path).is_some(),
            "and the row's own rectangle is not touched"
        );
    }

    /// A divider does not outlive its row. What `retain_live` drops for a node that has left the
    /// tree, it drops for that node's gaps too — otherwise a line survives the row it was drawn
    /// in, and a reader that asks the map rather than the tree finds a divider where there is no
    /// row.
    #[test]
    fn a_divider_does_not_outlive_its_row() {
        let state = DockState::new(vec![0u32]);
        let live = NodePath::new(SurfaceIndex::main(), state.main_surface().root().unwrap());
        let dead = NodePath::new(SurfaceIndex::main(), NodeId::new(9, 0));
        let mut layout = DockLayout::default();
        for path in [live, dead] {
            layout.set_rect(path, a_rect());
            layout.set_divider(gap(path, 0), Some(a_rect()));
        }

        layout.retain_live(&state);

        assert_eq!(
            layout.rect(dead),
            None,
            "control: the dead node's rectangle is dropped"
        );
        assert_eq!(layout.divider(gap(dead, 0)), None, "and so is its divider");
        assert!(
            layout.rect(live).is_some() && layout.divider(gap(live, 0)).is_some(),
            "while a live node keeps both"
        );
    }

    /// Forgetting a node — the inside of a stowed side — forgets every gap it had, and only its
    /// own: a row of three inside a stowed side leaves two lines behind otherwise, and the
    /// neighbour's line is not the stowed side's to take.
    #[test]
    fn forgetting_a_row_forgets_every_gap_it_had() {
        let row = NodePath::new(SurfaceIndex::main(), NodeId::new(3, 0));
        let neighbour = NodePath::new(SurfaceIndex::main(), NodeId::new(4, 0));
        let mut layout = DockLayout::default();
        for path in [row, neighbour] {
            layout.set_rect(path, a_rect());
        }
        for index in 0..2 {
            layout.set_divider(gap(row, index), Some(a_rect()));
        }
        layout.set_divider(gap(neighbour, 0), Some(a_rect()));

        layout.forget(row);

        assert_eq!(
            layout.rect(row),
            None,
            "control: the node itself is forgotten"
        );
        assert_eq!(layout.divider(gap(row, 0)), None);
        assert_eq!(
            layout.divider(gap(row, 1)),
            None,
            "both gaps, not only the first"
        );
        assert!(
            layout.divider(gap(neighbour, 0)).is_some(),
            "a neighbour's divider is not touched"
        );
    }
}
