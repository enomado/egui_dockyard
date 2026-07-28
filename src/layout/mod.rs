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
//! So geometry now lives here, keyed by [`NodePath`] (`surface` + `node`), and is stored
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
//! # use egui_dock::{DockLayout, NodePath};
//! # egui::__run_test_ctx(|ctx| {
//! let dock_id = egui::Id::new("egui_dock::DockArea");
//! let layout = DockLayout::load(ctx, dock_id);
//! let _rect = layout.rect(NodePath::MAIN_ROOT);
//! # });
//! ```
//!
//! Entries survive across frames (a node keeps its last known rectangle, exactly as the
//! old fields did), but nodes that no longer exist are dropped at the end of each pass —
//! see [`DockLayout::retain_live`].

mod convert;

use std::collections::HashMap;

use egui::{Context, Id, Rect};

use crate::{DockState, NodePath};

/// Where a single node ended up on screen during the last layout pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeGeometry {
    /// The full rectangle the node occupies: for a leaf, tab bar plus tab body.
    pub rect: Rect,

    /// The tab body rectangle, i.e. `rect` minus the tab bar. Only ever set for leaves,
    /// and only for leaves that actually rendered a body this frame (a collapsed or
    /// zero-sized leaf has no viewport).
    pub viewport: Option<Rect>,
}

/// Geometry of every node of a [`DockState`], as computed by the last layout pass.
///
/// Stored in [`egui::Context`] memory under the [`DockArea`](crate::DockArea)'s id, not
/// in the dock state — it is derived data and is deliberately never serialized.
#[derive(Clone, Debug, Default)]
pub struct DockLayout {
    nodes: HashMap<NodePath, NodeGeometry>,
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
    #[inline]
    pub(crate) fn set_rect(&mut self, path: NodePath, rect: Rect) {
        self.nodes
            .entry(path)
            .and_modify(|geometry| geometry.rect = rect)
            .or_insert(NodeGeometry {
                rect,
                viewport: None,
            });
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
            });
    }

    /// Forget the geometry of nodes that no longer exist in `dock_state`.
    ///
    /// Without this the map would grow forever: node indices are positional, so closing
    /// a tab leaves entries pointing at slots that are now empty (or, worse, reused by an
    /// unrelated node before the next layout pass overwrites them).
    pub(crate) fn retain_live<Tab>(&mut self, dock_state: &DockState<Tab>) {
        self.nodes.retain(|path, _| {
            dock_state
                .get_surface(path.surface)
                .and_then(|surface| surface.node_tree())
                .is_some_and(|tree| path.node.0 < tree.len() && !tree[path.node].is_empty())
        });
    }
}
