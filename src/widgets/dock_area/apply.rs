//! The second half of a frame: what drawing asked for, and putting it on the tree.
//!
//! Drawing reads the tree and writes nothing to it; everything it wants changed it says out loud
//! as a [`DockMutation`]. This module is where those requests are carried out of the pass and
//! applied — the only place in the crate that edits a [`DockState`] on a frame's behalf.

use std::collections::VecDeque;

use egui::{Context, Id, Pos2, Rect, Vec2};

use super::show::Geometry;
use super::state::DragInFlight;
use super::tab_removal::{ForcedRemoval, TabRemoval};
use super::{DockAreaResponse, DockEvent, DockMutation};
use crate::layout::DockLayout;
use crate::tab_viewer::OnCloseResponse;
use crate::{
    NodePath, RowGap, Style, SurfaceIndex, TabDestination, TabInsert, TabViewer, core::DockState,
};

/// One drawn frame of a [`DockArea`](super::DockArea), before it has been put on the tree.
///
/// Drawing borrows the tree **shared**, so the edits a frame asks for cannot be made while it is
/// being drawn — they are collected here and applied by [`apply`](Self::apply), which is the one
/// place holding the tree mutably. What is carried is therefore whatever applying needs and
/// nothing else, **by value**: this outlives the borrow it came from on purpose, and a field
/// pointing back into the tree would make `apply(&mut tree)` fail to compile.
///
/// The geometry of the frame travels with it for the same reason it is applied afterwards rather
/// than before: [`DockMutation::TransposeCross`] rewrites part of the tree, and the map has to be
/// brought back in step with the shape that was just written before anybody outside the frame
/// reads it.
#[must_use = "a frame that is not applied makes no edit and stores no geometry: the next frame \
              would draw the tree as it was and find no rectangles for it"]
pub struct DockDraw {
    /// The id the pass drew under, which is also where its geometry is kept.
    id: Id,
    style: Style,
    collapse_sideways: bool,
    /// Geometry of every node as this pass cut it, stored at the end of [`Self::apply`].
    layout: DockLayout,
    events: Vec<DockEvent>,
    mutations: Vec<DockMutation>,
    /// Where the pointer was, for a detach that has to put the new window somewhere.
    last_hover_pos: Option<Pos2>,
    pixels_per_point: f32,
    /// The drag still in flight at the end of the pass, reported back in the response.
    dragging: Option<DragInFlight>,
}

impl DockDraw {
    /// Everything the pass needs to hand over, gathered where the pass ends.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::widgets::dock_area) fn new(
        id: Id,
        style: Style,
        collapse_sideways: bool,
        layout: DockLayout,
        events: Vec<DockEvent>,
        mutations: Vec<DockMutation>,
        last_hover_pos: Option<Pos2>,
        pixels_per_point: f32,
        dragging: Option<DragInFlight>,
    ) -> Self {
        Self {
            id,
            style,
            collapse_sideways,
            layout,
            events,
            mutations,
            last_hover_pos,
            pixels_per_point,
            dragging,
        }
    }

    /// Make the edits this frame asked for, and publish the geometry it measured.
    ///
    /// The order of the two is not a detail: an edit can change the shape of the tree, so the map
    /// is trimmed and stored **after** the edits, or an out-of-frame reader would find a rectangle
    /// for a node that is gone — or none for a node that has just appeared.
    pub fn apply<Tab>(
        mut self,
        ctx: &Context,
        tree: &mut DockState<Tab>,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
    ) -> DockAreaResponse {
        let mutations = std::mem::take(&mut self.mutations);
        self.apply_mutations(&mutations, tree, tab_viewer);

        // Drop geometry of nodes that this pass removed (closed tabs, collapsed splits)
        // before publishing, so out-of-frame readers never see a rectangle for a node
        // that no longer exists.
        self.layout.retain_live(tree);
        std::mem::take(&mut self.layout).store(ctx, self.id);
        DockAreaResponse {
            events: self.events,
            dragging: self.dragging,
        }
    }

    /// Apply the requests accumulated while surfaces were rendered.
    ///
    /// This is deliberately a separate phase: draw code is allowed to *request* a structural
    /// edit, but it cannot invalidate paths while sibling surfaces are still being visited.
    fn apply_mutations<Tab>(
        &mut self,
        mutations: &[DockMutation],
        tree: &mut DockState<Tab>,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
    ) {
        let mut new_focused = mutations.iter().rev().find_map(|mutation| match mutation {
            DockMutation::Focus(path) => Some(*path),
            DockMutation::Activate(_)
            | DockMutation::SetLeafFold { .. }
            | DockMutation::SetSplitStowed { .. }
            | DockMutation::SetLeafScroll { .. }
            | DockMutation::SetBoundary { .. }
            | DockMutation::SetShares { .. }
            | DockMutation::SetWindowMinimized { .. }
            | DockMutation::WindowShown { .. }
            | DockMutation::TransposeCross { .. }
            | DockMutation::MoveTab { .. }
            | DockMutation::Remove(_)
            | DockMutation::Detach(_) => None,
        });

        // What a leaf *shows* is settled first, and in request order — these edits address nodes
        // that still exist exactly as drawing saw them. Removals come after, for the reason
        // spelled out on `DockMutation::Activate`: a removal asks who inherits the focus only
        // when it takes the active tab, so it has to see the activation this frame requested.
        for mutation in mutations {
            match *mutation {
                DockMutation::Activate(path) => {
                    let leaf = tree.leaf_mut(path.node_path()).unwrap();
                    if !leaf.is_active(path.tab) {
                        leaf.activate_tab_remembering(path.tab);
                        self.events.push(DockEvent::LayoutCommitted);
                    }
                }
                DockMutation::TransposeCross {
                    outer,
                    at,
                    ref bounds,
                    stack_fraction,
                } => {
                    let [first, second] = bounds;
                    tree[outer.row.surface].transpose_cross(
                        RowGap {
                            row: outer.row.node,
                            gap: outer.gap,
                        },
                        at,
                        [first, second],
                        stack_fraction,
                    );
                    // The pass drew — and hit-tested — the grouping this replaces, so the
                    // geometry map describes that one. Bring it back in step here, while the
                    // shape just written is the shape the surface has: `max_rect` is the surface
                    // root's rectangle, the same value `render_nodes` hands to
                    // `compute_rect_sizes`. Parents before children, each call cutting its
                    // children out of a rectangle an earlier call wrote.
                    let root = tree[outer.row.surface]
                        .root()
                        .expect("the surface being laid out has a root: `outer` lives in it");
                    let max_rect = self
                        .layout
                        .rect(NodePath::new(outer.row.surface, root))
                        .expect("the root was laid out at the top of this pass");
                    let mut queue = VecDeque::from([outer.row.node]);
                    while let Some(node) = queue.pop_front() {
                        let Some(children) = tree[outer.row.surface].children(node) else {
                            continue;
                        };
                        // Queued before the cut below, and only so that the borrow of the tree
                        // ends first — the queue is drained in exactly the order it was.
                        queue.extend(children.iter().copied());
                        self.compute_rect_sizes(
                            tree,
                            NodePath::new(outer.row.surface, node),
                            max_rect,
                        );
                    }
                }
                DockMutation::SetLeafFold { path, fold } => {
                    // Compared as a whole, axis included: re-folding a bar into a strip changes
                    // nothing about *whether* the leaf is folded and everything about the
                    // picture, so asking `is_collapsed` here would drop that request.
                    if tree[path].fold() != fold {
                        tree[path.surface].set_leaf_fold(path.node, fold);
                        // Reads the collapsed flag it has just written, plus this pass's
                        // geometry, to remember the height an expand has to restore.
                        self.window_update_collapsed(tree, path);
                        self.events.push(DockEvent::LayoutCommitted);
                    }
                }
                DockMutation::SetSplitStowed { path, stowed } => {
                    // Asked of `is_stowed`, not of `is_collapsed`: a side whose leaves all
                    // happen to be collapsed is collapsed without being stowed, and answering
                    // the wrong question here would drop the request that puts it away.
                    if tree[path].is_stowed() != stowed {
                        tree[path.surface].set_split_stowed(path.node, stowed);
                        // Same reason as for a collapsed leaf: in a floating window this is what
                        // the window's height follows, and stowing changes whether the root of
                        // that window is collapsed.
                        self.window_update_collapsed(tree, path);
                        self.events.push(DockEvent::LayoutCommitted);
                    }
                }
                DockMutation::SetLeafScroll { path, scroll } => {
                    tree.leaf_mut(path)
                        .expect("a scroll is only requested for a leaf")
                        .scroll = scroll;
                    // No `LayoutCommitted`: scrolling a tab bar has never been a layout edit a
                    // consumer diffs, and it is requested on plain resizes too (the clamp).
                }
                DockMutation::SetBoundary { gap, at } => {
                    tree[gap.row.surface][gap.row.node]
                        .get_row_mut()
                        .expect("a boundary is only requested for a row")
                        .set_boundary(gap.gap, at);
                    // No event either: the gesture that asked already said what it was —
                    // `SeparatorDragging` while the hand moves, `LayoutCommitted` on release or
                    // on the double-click reset.
                }
                DockMutation::SetShares { row, ref shares } => {
                    // Cloned rather than moved: the request list is borrowed for the whole loop
                    // (a `TransposeCross` further down reads its own `bounds` the same way), and
                    // a row's worth of weights is a handful of floats.
                    tree[row.surface][row.node]
                        .get_row_mut()
                        .expect("weights are only requested for a row")
                        .set_shares(shares.clone());
                    // No event, for the same reason `SetBoundary` pushes none: the gesture that
                    // asked has already said what it was.
                }
                DockMutation::SetWindowMinimized { surface, minimized } => {
                    // Pushes `LayoutCommitted` itself, as it did when it ran during the click.
                    self.window_set_minimized(tree, surface, minimized);
                }
                DockMutation::WindowShown {
                    surface,
                    took_expanded_height,
                } => {
                    tree.get_window_state_mut(surface)
                        .expect("the window was drawn this frame")
                        .requests_honoured(took_expanded_height);
                }
                DockMutation::MoveTab { .. }
                | DockMutation::Remove(_)
                | DockMutation::Detach(_)
                | DockMutation::Focus(_) => (),
            }
        }

        for mutation in mutations.iter().rev() {
            let DockMutation::Remove(removal) = *mutation else {
                continue;
            };
            match removal {
                TabRemoval::Tab(path, ForcedRemoval(is_forced)) => {
                    // Who takes the focus is the application's call when it wants it; asked
                    // only for the tab that has it, since closing any other moves nothing.
                    // The leaf is handed over as it stands now — the removal is next.
                    let successor = {
                        let leaf = tree.leaf(path.node_path()).unwrap();
                        leaf.is_active(path.tab)
                            .then(|| tab_viewer.successor_on_close(leaf, path.tab))
                            .flatten()
                    };
                    if is_forced {
                        tree.remove_tab_choosing(path, successor);
                        self.events.push(DockEvent::LayoutCommitted);
                    } else {
                        let leaf = &mut tree.leaf_mut(path.node_path()).unwrap();
                        match tab_viewer.on_close(&leaf[path.tab]) {
                            OnCloseResponse::Close => {
                                tree.remove_tab_choosing(path, successor);
                                self.events.push(DockEvent::LayoutCommitted);
                            }
                            OnCloseResponse::Focus => {
                                leaf.activate_tab_remembering(path.tab);
                                new_focused = Some(path.node_path());
                                self.events.push(DockEvent::LayoutCommitted);
                            }
                            OnCloseResponse::Ignore => {
                                // no-op
                            }
                        }
                    }
                }
                TabRemoval::Node(path) => {
                    let mut all_tabs_are_closable = true;
                    for tab in tree[path].iter_tabs_mut() {
                        if !(tab_viewer.is_closeable(tab)
                            && matches!(tab_viewer.on_close(tab), OnCloseResponse::Close))
                        {
                            all_tabs_are_closable = false;
                        }
                    }
                    if all_tabs_are_closable {
                        tree.remove_leaf(path);
                        self.events.push(DockEvent::LayoutCommitted);
                    }
                }
                TabRemoval::Window(window) => {
                    let mut all_tabs_are_closable = true;
                    for node in tree[SurfaceIndex::Window(window)].iter_mut() {
                        for tab in node.iter_tabs_mut() {
                            if !(tab_viewer.is_closeable(tab)
                                && matches!(tab_viewer.on_close(tab), OnCloseResponse::Close))
                            {
                                all_tabs_are_closable = false;
                            }
                        }
                    }
                    if all_tabs_are_closable {
                        tree.remove_window(window);
                        self.events.push(DockEvent::LayoutCommitted);
                    }
                }
            }
        }

        for mutation in mutations.iter().rev() {
            let DockMutation::Detach(path) = *mutation else {
                continue;
            };
            // The detached window inherits the size of the node the tab came from; a
            // node that was never laid out (nothing to inherit) gets a default size.
            let size = self
                .layout
                .rect(path.node_path())
                .map_or(Vec2::new(100., 150.), |rect| rect.size());
            tree.detach_tab(
                path,
                Rect::from_min_size(self.last_hover_pos.unwrap_or(Pos2::ZERO), size).into(),
            );
            self.events.push(DockEvent::LayoutCommitted);
        }

        // The drop, after everything drawing asked for: see `DockMutation::MoveTab` for why this
        // order and not the other one, and why the subject is re-resolved here.
        for mutation in mutations {
            let DockMutation::MoveTab {
                source,
                destination,
            } = *mutation
            else {
                continue;
            };
            // A drag whose tab has left the tree was cancelled before the pass began — but the
            // pass itself can also have taken it, by a forced close asked for while drawing.
            let Some(source) = source.resolve(tree) else {
                continue;
            };
            if !destination_is_live(tree, destination) {
                continue;
            }
            // A drop that resolves to the tab's current slot changes nothing; only a move that
            // reports a real mutation counts as a finalised event (same rule as the focus push
            // below).
            if tree.move_tab(source, destination) {
                self.events.push(DockEvent::LayoutCommitted);
            }
        }

        if let Some(focused) = new_focused {
            // `new_focused` is set unconditionally on any click within a leaf
            // body and on tab-title clicks, even when the leaf is already
            // focused. Only emit a finalised event if the focus actually
            // moved — otherwise idle clicks inside already-focused leaves
            // would emit empty events.
            let already_focused = tree.focused_leaf() == Some(focused);
            tree.set_focused_node_and_surface(focused);
            if !already_focused {
                self.events.push(DockEvent::LayoutCommitted);
            }
        }
    }

    /// Bring the geometry of `path`'s children back in step with the tree as it now stands.
    ///
    /// The measuring and the writing are two steps because the tree is held mutably here: see
    /// [`RowPlan`], which is what carries the answer between them.
    fn compute_rect_sizes<Tab>(&mut self, tree: &DockState<Tab>, path: NodePath, max_rect: Rect) {
        let plan = Geometry::new(tree, &self.layout, &self.style, self.collapse_sideways)
            .plan_row(self.pixels_per_point, path, max_rect);
        plan.write(&mut self.layout);
    }

    /// Updates the collapsed state of the node and its parents.
    ///
    /// Called right after the collapsed flag it reads has been written, not from the click
    /// handler that requested the change.
    fn window_update_collapsed<Tab>(&mut self, tree: &mut DockState<Tab>, path: NodePath) {
        let surface = &mut tree[path.surface];
        let collapsed = surface[path.node].is_collapsed();
        if !collapsed {
            if let Some(window_state) = tree.get_window_state_mut(path.surface) {
                window_state.set_new(true);
            }
        } else if surface.root_node().is_some_and(|root| root.is_collapsed()) {
            // Height of the window before collapsing, so expanding restores it. A root
            // that was never laid out has no height to remember.
            let surface_height = surface
                .root()
                .and_then(|root| self.layout.rect(NodePath::new(path.surface, root)))
                .map_or(0.0, |rect| rect.height());
            if let Some(window_state) = tree.get_window_state_mut(path.surface) {
                window_state.set_expanded_height(surface_height);
            }
        }
    }

    /// Minimize or restore a window, applied from [`DockMutation::SetWindowMinimized`].
    ///
    /// Reads this pass's geometry to remember how tall the window was, exactly as it did when
    /// it ran during the click.
    fn window_set_minimized<Tab>(
        &mut self,
        tree: &mut DockState<Tab>,
        surf_index: SurfaceIndex,
        minimized: bool,
    ) {
        let was_minimized = tree.get_window_state(surf_index).unwrap().is_minimized();
        if was_minimized == minimized {
            return;
        }
        let surface = &mut tree[surf_index];

        if surface.root_node().is_some_and(|node| node.is_collapsed()) {
            // The window is already fully collapsed,
            // so `expanded_height` has already been set.
            // We don't need to set `new` either.
            if let Some(window_state) = tree.get_window_state_mut(surf_index) {
                window_state.toggle_minimized();
            }
        } else if was_minimized {
            if let Some(window_state) = tree.get_window_state_mut(surf_index) {
                window_state.set_new(true);
                window_state.toggle_minimized();
            }
        } else {
            // Remember how tall the window was so un-minimizing restores that height. A
            // surface that was never laid out has no height to remember.
            let surface_height = tree[surf_index]
                .root()
                .and_then(|root| self.layout.rect(NodePath::new(surf_index, root)))
                .map_or(0.0, |rect| rect.height());
            if let Some(window_state) = tree.get_window_state_mut(surf_index) {
                window_state.set_expanded_height(surface_height);
                window_state.toggle_minimized();
            }
        }
        self.events.push(DockEvent::LayoutCommitted);
    }
}

/// Whether a destination settled before this pass drew still names somewhere to drop onto.
///
/// A destination is a *path*, and the requests applied ahead of the drop can have taken what
/// it names away — or, for [`TabInsert::Insert`], shortened the bar the slot was counted in.
/// Asked once, here, rather than left to [`DockState::move_tab`], which indexes on the
/// assumption that whoever called it aimed at a tree that still stands.
fn destination_is_live<Tab>(tree: &DockState<Tab>, destination: TabDestination) -> bool {
    match destination {
        // Made by the drop itself, so there is nothing for it to outlive.
        TabDestination::Window(_) => true,
        TabDestination::EmptySurface(surface) => tree.is_surface_valid(surface),
        TabDestination::Node(path, insert) => match insert {
            TabInsert::Insert(index) => tree
                .leaf(path)
                // `<=`, not `<`: the far edge of the bar is a slot like any other, and a
                // removal that shortens the bar to exactly the aimed-at index leaves the
                // drop meaning "at the end", which is where the hand was pointing.
                .is_ok_and(|leaf| index.0 <= leaf.len()),
            TabInsert::Append | TabInsert::Split(_) => tree.node(path).is_ok(),
        },
    }
}
