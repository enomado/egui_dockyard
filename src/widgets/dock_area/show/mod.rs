use duplicate::duplicate;
use egui::{
    Context, CornerRadius, CursorIcon, EventFilter, Key, Pos2, Rect, Sense, StrokeKind, Ui, Vec2,
    epaint::MarginF32,
};
use paste::paste;

use super::{
    DockAreaResponse, DockMutation,
    drag_and_drop::{DragSource, HoverData, overlay_layer, register_overlay_layer},
    events::DockEvent,
    state::{DragSubject, State},
    tab_removal::TabRemoval,
};
use crate::NodePath;
use crate::dock_area::tab_removal::ForcedRemoval;
use crate::layout::DockLayout;
use crate::tab_viewer::OnCloseResponse;
use crate::{
    AllowedSplits, DockArea, Node, OverlayType, SeparatorStyle, Style, SurfaceIndex,
    TabDestination, TabViewer,
    utils::{expand_to_pixel, fade_dock_style, map_to_pixel},
};

mod junction;
mod leaf;
mod main_surface;
mod window_surface;

impl<Tab> DockArea<'_, Tab> {
    /// Shows the docking hierarchy inside a [`Ui`].
    ///
    /// See also [`show`](Self::show) and
    /// [`show_inside_with_response`](Self::show_inside_with_response).
    #[inline]
    pub fn show_inside(self, ui: &mut Ui, tab_viewer: &mut impl TabViewer<Tab = Tab>) {
        let _ = self.show_inside_with_response(ui, tab_viewer);
    }

    /// Same as [`show_inside`](Self::show_inside) but returns a
    /// [`DockAreaResponse`] describing what changed during this render pass.
    pub fn show_inside_with_response(
        mut self,
        ui: &mut Ui,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
    ) -> DockAreaResponse {
        self.style
            .get_or_insert(Style::from_egui(ui.style().as_ref()));
        self.window_bounds.get_or_insert(ui.ctx().content_rect());

        // Before anything is drawn, and every pass: the drop overlay's layer takes its rank among
        // the foreground areas here rather than when a drag starts. See `register_overlay_layer`.
        register_overlay_layer(ui, self.id);

        let mut state = State::load(ui.ctx(), self.id);
        // Last frame's geometry: the layout pass below overwrites every live node, but
        // the tab body compares against the previous viewport to decide whether to call
        // `TabViewer::on_rect_changed`, so we start from what was there.
        self.layout = DockLayout::load(ui.ctx(), self.id);

        // Delay hover position one frame. On touch screens hover_pos() is None when any_released()
        if !ui.input(|i| i.pointer.any_released()) {
            state.last_hover_pos = ui.input(|i| i.pointer.hover_pos());
        }

        let hover_data =
            ui.memory_mut(|mem| mem.data.remove_temp(self.id.with("hover_data")).flatten());

        // A drag carries a tab, and that tab can leave the tree while the hand is still
        // holding it: middle-click closes a tab, and the dragged tab is still a tab in the
        // bar — but the application may equally rewrite the `DockState` between two frames.
        // Whatever the route, a drag of a tab that no longer exists is over, and this is the
        // one place that says so.
        //
        // Both halves have to go. The dock's own drag state is dropped here; egui's is
        // stopped too, because the id a tab is drawn under names a *position* in the bar, so
        // the neighbour that slides into the closed tab's slot inherits the id — and, with
        // it, a drag nobody started on it.
        //
        // A drag ends the other way round as well, and that half was missing. **Any** release
        // ends egui's drag, and only the *primary* one is a drop here — so a middle click,
        // which is how a tab is closed, leaves the hand closed over a dock that still believes
        // it is carrying a tab. Nothing followed it visually — back then no overlay could be
        // drawn, because the leaf stops publishing the moment egui lets go — which is precisely
        // why it survived: the only witness was `dragged_tab`, answering with a tab that was
        // going nowhere. The dock's drag exists while egui's does, and that is now stated
        // rather than assumed — by this test, not by a rectangle failing to arrive.
        //
        // Asked of the one place that says what the hand holds, and asked once: the source used
        // to be carried in two more places besides — a temp channel the leaf published into,
        // and the open `DragDropState` — and each of them had to be checked for the same decay
        // separately.
        let primary_down = ui.input(|i| i.pointer.primary_down());
        let carried = state.carried_tab();
        let drag_is_over = |src: &DragSource| {
            src.resolve(self.dock_state).is_none()
                // Guarded on the button still being down, because the *ordinary* end of a drag
                // looks exactly like this: egui stops dragging on the frame the primary comes
                // up, and that frame is the drop — resolved a dozen lines below.
                || (primary_down
                    && !ui.ctx().is_being_dragged(crate::tab_widget_id(
                        self.id,
                        src.node_path(),
                        src.tab,
                    )))
        };
        let carried = carried.filter(|src| {
            if drag_is_over(src) {
                state.reset_drag();
                ui.ctx().stop_dragging();
                false
            } else {
                true
            }
        });
        // The carried tab's leaf as it stood last frame, while the tab was still part of it —
        // the size a "drop into a window" preview is drawn at. Geometry, and only geometry:
        // whether a drag exists, whose it is, and whether it has been pulled far enough out of
        // the bar for a drop to resolve at all are three things the field answers, the last of
        // them `DragInFlight::moved`.
        //
        // Asked of the geometry map rather than of a temp channel the leaf published into. The
        // two are the same rectangle by construction, not by coincidence: `render_nodes` cuts
        // every node's rectangle before any leaf draws, so what the leaf used to publish was
        // read out of this very map on the frame it published, and the map is stored at the end
        // of that pass for this one to load. Measured before the channel was deleted — the
        // published value and this one were asserted equal across the sweep and every directed
        // drag scene, and a one-pixel translation of it reddened the sweep and twelve tests by
        // name.
        let pulled_out = state.in_flight().is_some_and(|drag| drag.moved);
        let source_rect = carried
            .filter(|_| pulled_out)
            .and_then(|src| self.layout.rect(src.node_path()));

        // The hover's destination decays the same way the drag's source does, and for the same
        // reason: it addresses a *node*, and a node can leave the tree while this drag is
        // still open. Two places carry a destination, and both have to be checked, the same
        // way the drag's own source is above:
        //
        // * `hover_data`, just read out of this frame's temp storage, is a full frame stale by
        //   construction — it was published by whichever leaf was under the pointer *last*
        //   frame, while rendering itself. A leaf that closes itself this same pass (a force
        //   close driven by the application) still runs that publish before the close takes
        //   effect, so the value sitting in memory for this frame to read can already name a
        //   node that is gone by the time it is read.
        // * `state.dnd.hover`, held over from a previous frame, decays the way `state.dnd.drag`
        //   does above — except there is no lock on the drag source, while the destination has
        //   one (`is_drag_drop_locked`) precisely to hold a preference steady while the pointer
        //   settles. That mechanism cannot tell "steady because nothing changed" from "steady
        //   because what it named is gone" — they look identical to it — so the address can
        //   stay locked in on a dead node for as long as the lock does.
        //
        // Either one reaching `show_drag_drop_overlay`/`move_tab` names a node the tree does
        // not have, which is where it used to panic (see FINDINGS.md, "no node 1.0 in this
        // tree").
        let hover_data =
            hover_data.filter(|hover: &HoverData| !hover.dst.node_is_gone(self.dock_state));
        if let Some(dnd) = state.dnd.as_mut()
            && dnd
                .hover
                .as_ref()
                .is_some_and(|hover| hover.dst.node_is_gone(self.dock_state))
        {
            // Dropped rather than merely skipped: `set_drag_and_drop` below only *writes* a
            // fresh preference when the current one is `None` or unlocked, so leaving the stale
            // one in place would keep it alive — and stale — for the rest of the lock window.
            // Clearing it here lets a live preference (`hover_data`, just filtered above) take
            // over immediately, this same frame, if there is one; if there is not, this is
            // exactly the ordinary "nothing hovered yet" state every frame without a preference
            // is already in.
            //
            // Only `.hover`, not the whole `DragDropState`: the drag's *source* never went
            // stale here, only the destination did, and dropping `dnd` entirely used to end the
            // drag along with it. A DST sweep run caught the consequence — closing the leaf a
            // hold had settled its preference on left `ctx.dragged_id()` and the dock's own
            // `dragged_tab` disagreeing for a frame, because the fix silently ended a drag that
            // was still live everywhere else.
            dnd.drop_stale_hover();
        }

        if let (Some(source_rect), Some(hover)) = (source_rect, hover_data) {
            let style = self.style.as_ref().unwrap();
            state.set_drag_and_drop(source_rect, hover, ui.ctx(), style);
            let tab_dst = self.show_drag_drop_overlay(ui, &mut state, carried.unwrap(), tab_viewer);
            if ui.input(|i| i.pointer.primary_released())
                && let Some(destination) = tab_dst
            {
                // Resolved against the tree as it stands, not as it stood when the drag
                // started: the leaf may have been edited in between, and the drop has to
                // move the tab the hand grabbed rather than whatever now sits at its old
                // index. `None` cannot happen — a source that stopped resolving ended the
                // drag above — so it is an assertion, not a branch.
                let source = carried
                    .expect("the overlay only resolves a destination for a carried tab")
                    .resolve(self.dock_state)
                    .expect("a drag whose tab is gone was already cancelled");
                // A drop that resolves to the tab's current slot changes nothing; only a
                // move that reports a real mutation counts as a finalised event (same rule
                // as the focus push at the end of this pass).
                if self.dock_state.move_tab(source, destination) {
                    self.events.push(DockEvent::LayoutCommitted);
                }
            }
        }

        if ui.input(|i| i.pointer.primary_released()) {
            state.reset_drag();
        }

        let style = self.style.as_ref().unwrap();
        let fade_surface =
            self.hovered_window_surface(&mut state, style.overlay.feel.fade_hold_time, ui.ctx());
        let fade_style = {
            fade_surface.is_some().then(|| {
                let mut fade_style = style.clone();
                fade_dock_style(&mut fade_style, style.overlay.surface_fade_opacity);
                (fade_style, style.overlay.surface_fade_opacity)
            })
        };

        for &surface_index in self.dock_state.valid_surface_indices().iter() {
            self.show_surface_inside(
                surface_index,
                ui,
                tab_viewer,
                &mut state,
                fade_style.as_ref().map(|(style, factor)| {
                    (style, *factor, fade_surface.unwrap_or(SurfaceIndex::main()))
                }),
            );
        }

        let mutations = std::mem::take(&mut self.mutations);
        self.apply_render_mutations(&mutations, state.last_hover_pos, tab_viewer);

        // Read before the state is stored away, and read through the liveness filter for the same
        // reason `drag_in_flight` does: a gesture whose subject left the tree never gets its
        // `drag_stopped`, and what it leaves behind is a leftover the dock itself no longer acts
        // on — announcing it would name a gesture nobody is making.
        let dragging = state.in_flight_at(ui.ctx().cumulative_pass_nr()).copied();
        state.store(ui.ctx(), self.id);
        // Drop geometry of nodes that this pass removed (closed tabs, collapsed splits)
        // before publishing, so out-of-frame readers never see a rectangle for a node
        // that no longer exists.
        self.layout.retain_live(self.dock_state);
        std::mem::take(&mut self.layout).store(ui.ctx(), self.id);
        DockAreaResponse {
            events: std::mem::take(&mut self.events),
            dragging,
        }
    }

    /// Apply the requests accumulated while surfaces were rendered.
    ///
    /// This is deliberately a separate phase: draw code is allowed to *request* a structural
    /// edit, but it cannot invalidate paths while sibling surfaces are still being visited.
    /// The method is the pre-public D3 seam; D4 moves the remaining live edits into the same
    /// request list before the draw API itself can borrow only `&DockState`.
    fn apply_render_mutations(
        &mut self,
        mutations: &[DockMutation],
        last_hover_pos: Option<Pos2>,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
    ) {
        let mut new_focused = mutations.iter().rev().find_map(|mutation| match mutation {
            DockMutation::Focus(path) => Some(*path),
            DockMutation::Remove(_) | DockMutation::Detach(_) => None,
        });

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
                        let leaf = self.dock_state.leaf(path.node_path()).unwrap();
                        leaf.is_active(path.tab)
                            .then(|| tab_viewer.successor_on_close(leaf, path.tab))
                            .flatten()
                    };
                    if is_forced {
                        self.dock_state.remove_tab_choosing(path, successor);
                        self.events.push(DockEvent::LayoutCommitted);
                    } else {
                        let leaf = &mut self.dock_state.leaf_mut(path.node_path()).unwrap();
                        match tab_viewer.on_close(&mut leaf[path.tab]) {
                            OnCloseResponse::Close => {
                                self.dock_state.remove_tab_choosing(path, successor);
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
                    for tab in self.dock_state[path].iter_tabs_mut() {
                        if !(tab_viewer.is_closeable(tab)
                            && matches!(tab_viewer.on_close(tab), OnCloseResponse::Close))
                        {
                            all_tabs_are_closable = false;
                        }
                    }
                    if all_tabs_are_closable {
                        self.dock_state.remove_leaf(path);
                        self.events.push(DockEvent::LayoutCommitted);
                    }
                }
                TabRemoval::Window(window) => {
                    let mut all_tabs_are_closable = true;
                    for node in self.dock_state[SurfaceIndex::Window(window)].iter_mut() {
                        for tab in node.iter_tabs_mut() {
                            if !(tab_viewer.is_closeable(tab)
                                && matches!(tab_viewer.on_close(tab), OnCloseResponse::Close))
                            {
                                all_tabs_are_closable = false;
                            }
                        }
                    }
                    if all_tabs_are_closable {
                        self.dock_state.remove_window(window);
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
            self.dock_state.detach_tab(
                path,
                Rect::from_min_size(last_hover_pos.unwrap_or(Pos2::ZERO), size).into(),
            );
            self.events.push(DockEvent::LayoutCommitted);
        }

        if let Some(focused) = new_focused {
            // `new_focused` is set unconditionally on any click within a leaf
            // body and on tab-title clicks, even when the leaf is already
            // focused. Only emit a finalised event if the focus actually
            // moved — otherwise idle clicks inside already-focused leaves
            // would emit empty events.
            let already_focused = self.dock_state.focused_leaf() == Some(focused);
            self.dock_state.set_focused_node_and_surface(focused);
            if !already_focused {
                self.events.push(DockEvent::LayoutCommitted);
            }
        }
    }

    /// Returns some when windows are fading, and what surface index is being hovered over
    #[inline(always)]
    fn hovered_window_surface(
        &self,
        state: &mut State,
        hold_time: f32,
        ctx: &Context,
    ) -> Option<SurfaceIndex> {
        if let Some(dnd_state) = &state.dnd
            && dnd_state.is_locked(self.style.as_ref().unwrap(), ctx)
            && let Some(hover) = dnd_state.hover.as_ref()
        {
            state.window_fade = Some((ctx.input(|i| i.time), hover.dst.surface_address()));
        }

        state.window_fade.and_then(|(time, surface)| {
            ctx.request_repaint();
            (hold_time > (ctx.input(|i| i.time) - time) as f32).then_some(surface)
        })
    }

    /// Resolve where a dragged tab would land given it's dropped this frame, returns `None` when the resulting drop is an invalid move.
    ///
    /// `carried` is what the hand holds, handed in by the caller that read it from the field
    /// rather than read again off `state` here: the destination half (`state.dnd`) is borrowed
    /// mutably for the length of this, and the subject is not part of it.
    fn show_drag_drop_overlay(
        &mut self,
        ui: &Ui,
        state: &mut State,
        carried: DragSource,
        tab_viewer: &impl TabViewer<Tab = Tab>,
    ) -> Option<TabDestination> {
        let drag_state = state.dnd.as_mut().unwrap();
        let style = self.style.as_ref().unwrap();
        // This is the one place `.hover` is unwrapped without a liveness check of its own — on
        // purpose: this function only ever runs right after `State::set_drag_and_drop` wrote a
        // fresh one this same frame (see the call site, gated on `hover_data` being fresh and
        // live). Cloned out to a local rather than read through `drag_state` from here on, so
        // the address itself — not "whatever `.hover` currently holds" — is what every branch
        // below agrees on, including `update_lock`'s own read of it.
        let hover = drag_state
            .hover
            .clone()
            .expect("show_drag_drop_overlay is only called with a freshly-set hover");

        let deserted_node = {
            let src = carried;
            match hover.dst.node_address() {
                (dst_surf, Some(dst_node)) => {
                    src.surface == dst_surf
                        && src.node == dst_node
                        && self.dock_state[src.node_path()].tabs_count() == 1
                }
                _ => false,
            }
        };

        // Not all scenarios can house all splits.
        let restricted_splits = if hover.dst.is_surface() || deserted_node {
            AllowedSplits::None
        } else {
            AllowedSplits::All
        };
        let allowed_splits = self.allowed_splits & restricted_splits;

        let allowed_in_window = {
            let path = carried
                .resolve(self.dock_state)
                .expect("a drag whose tab is gone was already cancelled");
            let Node::Leaf(leaf) = &mut self.dock_state[path.node_path()] else {
                unreachable!("tab drags can only come from leaf nodes")
            };
            tab_viewer.allowed_in_windows(&mut leaf[path.tab])
        };

        if let Some(pointer) = state.last_hover_pos {
            drag_state.pointer = pointer;
        }

        let window_bounds = self.window_bounds.unwrap();
        // Named, not registered: the area itself is shown once per pass from
        // `show_inside_with_response`, whether a drag is in flight or not, so that its rank among
        // the other foreground areas is taken before any menu the application opens later. See
        // `register_overlay_layer`.
        let overlay = overlay_layer(self.id);
        match (style.overlay.overlay_type, hover.tab.is_some()) {
            (OverlayType::HighlightedAreas, _) | (_, true) => drag_state.resolve_traditional(
                &hover,
                carried,
                ui,
                overlay,
                style,
                allowed_splits,
                allowed_in_window,
                window_bounds,
            ),
            (OverlayType::Widgets, false) => drag_state.resolve_icon_based(
                &hover,
                carried,
                ui,
                overlay,
                style,
                allowed_splits,
                allowed_in_window,
                window_bounds,
            ),
        }
    }

    /// Show a single surface of a [`DockState`].
    fn show_surface_inside(
        &mut self,
        surf_index: SurfaceIndex,
        ui: &mut Ui,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        state: &mut State,
        fade_style: Option<(&Style, f32, SurfaceIndex)>,
    ) {
        match surf_index {
            SurfaceIndex::Main => self.show_root_surface_inside(ui, tab_viewer, state),
            SurfaceIndex::Window(window) => {
                self.show_window_surface(ui, window, tab_viewer, state, fade_style)
            }
        }
    }

    fn render_nodes(
        &mut self,
        ui: &mut Ui,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        state: &mut State,
        surf_index: SurfaceIndex,
        fade_style: Option<(&Style, f32)>,
    ) {
        // Breadth-first: a node's rectangle is cut out of its parent's, so parents have to
        // be laid out first. The order is snapshotted once and reused by all three passes;
        // nothing below changes the shape of the tree.
        let order = self.dock_state[surf_index].breadth_first();

        // First compute all rect sizes in the node graph.
        let pixels_per_point = ui.ctx().pixels_per_point();
        let max_rect = self.allocate_area_for_root_node(ui, surf_index);
        for node in order.iter().copied() {
            let path = NodePath::new(surf_index, node);
            if self.dock_state[path].is_parent() {
                self.compute_rect_sizes(pixels_per_point, path, max_rect);
            }
        }

        // Then, draw the bodies of each leaves.
        for node in order.iter().copied() {
            let path = NodePath::new(surf_index, node);
            if self.dock_state[path].is_leaf() {
                self.show_leaf(ui, state, path, tab_viewer, fade_style);
            }
        }

        // Finally, draw separators so that their "interaction zone" is above
        // bodies (see `SeparatorStyle::extra_interact_width`).
        let fade_style = fade_style.map(|(style, _)| style);
        for node in order.iter().copied() {
            let path = NodePath::new(surf_index, node);
            if self.dock_state[path].is_parent() {
                self.show_separator(ui, path, fade_style, state);
            }
        }
    }

    /// The paths of a split's two children, first (left / top) then second (right / bottom).
    ///
    /// # Panics
    ///
    /// If `path` does not name a split.
    #[track_caller]
    fn child_paths(&self, path: NodePath) -> [NodePath; 2] {
        let [left, right] = self.dock_state[path]
            .get_split()
            .expect("only a split has children")
            .children();
        [
            NodePath::new(path.surface, left),
            NodePath::new(path.surface, right),
        ]
    }

    fn allocate_area_for_root_node(&mut self, ui: &mut Ui, surface: SurfaceIndex) -> Rect {
        let style = self.style.as_ref().unwrap();
        let mut rect = ui.available_rect_before_wrap();

        if let Some(margin) = style.dock_area_padding {
            rect.min += margin.left_top();
            rect.max -= margin.right_bottom();
        }

        ui.painter().rect_stroke(
            rect,
            style.main_surface_border_rounding,
            style.main_surface_border_stroke,
            StrokeKind::Inside,
        );
        // Start drawing inside the border, not on it — see `border_clearance`. Every surface,
        // not only the main one: the stroke is painted for all of them, and a window surface
        // used not to step back from it at all, so a bordered window drew its border and then
        // covered it with the first tab bar.
        rect -= border_clearance(style);
        ui.allocate_rect(rect, Sense::hover());

        let Some(root) = self.dock_state[surface].root() else {
            return rect;
        };
        self.layout.set_rect(NodePath::new(surface, root), rect);
        rect
    }

    /// Write the rectangles of `path`'s two children into [`Self::layout`], cutting them out
    /// of the rectangle already recorded for `path` itself.
    ///
    /// Takes `pixels_per_point` rather than a [`Ui`] because it is also called from
    /// [`Self::transpose_cross_split`], which edits the tree in the middle of a pass and has
    /// to bring the geometry map back in step with the new shape right there — see the note
    /// on staleness in that function.
    fn compute_rect_sizes(&mut self, pixels_per_point: f32, path: NodePath, max_rect: Rect) {
        assert!(self.dock_state[path].is_parent());

        let style = self.style.as_ref().unwrap();

        let [left_path, right_path] = self.child_paths(path);
        let left_collapsed_count = self.dock_state[left_path].collapsed_leaf_count();
        let right_collapsed_count = self.dock_state[right_path].collapsed_leaf_count();
        let left_collapsed = self.dock_state[left_path].is_collapsed();
        let right_collapsed = self.dock_state[right_path].is_collapsed();

        // The parent's rectangle was written either by `allocate_area_for_root_node` (for
        // the root) or by this same function when its own parent was visited — the
        // breadth-first order of the caller guarantees it is already there.
        let parent_rect = self
            .layout
            .rect(path)
            .expect("a parent node must have been laid out before its children");

        if (left_collapsed || right_collapsed)
            && self.dock_state[path.surface][path.node].is_vertical()
        {
            let rect = split_rect(parent_rect, pixels_per_point);

            // The collapsed side is not cut at a ratio — it is given exactly what its rows
            // need, and the divider goes *beside* that, not through it. It used to straddle
            // the boundary, taking half its width out of the collapsed rows: with the
            // boundary at `rows * tab_bar.height` the last row was drawn a hairline taller
            // than the space it had, and the whole strip was one divider short per row.
            //
            // Which end the strip is anchored to is the only difference between the two
            // cases, so all that differs below is where the divider's two edges land.
            let (near, far) = if left_collapsed {
                // EITHER only left collapsed OR both: the strip is the top of the node, and
                // the divider begins where it ends.
                let strip_end = rect.min.y + collapsed_strip_height(left_collapsed_count, style);
                (strip_end, strip_end + style.separator.width)
            } else {
                // Only right collapsed: the strip is the bottom of the node.
                let strip_start = rect.max.y - collapsed_strip_height(right_collapsed_count, style);
                (strip_start - style.separator.width, strip_start)
            };

            let left_separator_border = map_to_pixel(near, pixels_per_point, f32::round);
            let right_separator_border = map_to_pixel(far, pixels_per_point, f32::round);
            let left = rect
                .intersect(Rect::everything_above(left_separator_border))
                .intersect(max_rect);
            let right = rect
                .intersect(Rect::everything_below(right_separator_border))
                .intersect(max_rect);
            self.layout.set_rect(left_path, left);
            self.layout.set_rect(right_path, right);
            return;
        }

        duplicate! {
            [
                orientation   dim_point  dim_size  left_of    right_of;
                [Horizontal]  [x]        [width]   [left_of]  [right_of];
                [Vertical]    [y]        [height]  [above]    [below];
            ]
            // Copy the fraction out before touching `self.layout`: holding a borrow of
            // the node while writing the geometry map would borrow `self` twice.
            if let Node::orientation(split) = &self.dock_state[path.surface][path.node] {
                let fraction = split.fraction;
                let rect = split_rect(parent_rect, pixels_per_point);

                // The children are cut at where the boundary *is*, which is the stored ratio
                // pushed into the band this frame's geometry can honour — see `SeparatorBand`.
                // Clamping here, rather than writing the clamped number back into the tree,
                // is what lets a node with no room for the margin keep the ratio it will get
                // back as soon as there is room again.
                let dim_size = rect.dim_size();
                let band = SeparatorBand::new(fraction, dim_size, style.separator.extra);
                let midpoint = band.midpoint(rect.min.dim_point, dim_size);

                let left_separator_border = map_to_pixel(
                    midpoint - style.separator.width * 0.5,
                    pixels_per_point,
                    f32::round
                );
                let right_separator_border = map_to_pixel(
                    midpoint + style.separator.width * 0.5,
                    pixels_per_point,
                    f32::round
                );

                paste! {
                    let left = rect.intersect(Rect::[<everything_ left_of>](left_separator_border)).intersect(max_rect);
                    let right = rect.intersect(Rect::[<everything_ right_of>](right_separator_border)).intersect(max_rect);
                }

                self.layout.set_rect(left_path, left);
                self.layout.set_rect(right_path, right);
            }
        }
    }

    /// The rectangle the divider of the split at `path` is drawn in — and, expanded by
    /// [`SeparatorStyle::extra_interact_width`](crate::SeparatorStyle::extra_interact_width),
    /// grabbed by.
    ///
    /// `None` where there is no divider on screen to speak of: a node that is not a split, one
    /// the layout pass has no rectangle for, or a vertical split with a collapsed child — that
    /// last one is cut at the strip's edge rather than at its ratio, and `show_separator` does
    /// not draw or hit-test it at all.
    ///
    /// A function, and the only derivation of this rectangle in the crate, for the reason
    /// [`split_rect`] gives: the drawn divider, the rectangle it is grabbed by, and anything
    /// that needs to know where it *is* have to name the same line. The third of those is new —
    /// the cross-split button is sized by how close the nearest other divider is (see
    /// `DockArea::toggle_room`) — and re-deriving it there would have been the third copy of an
    /// arithmetic that has already drifted once.
    pub(super) fn separator_rect(
        &self,
        path: NodePath,
        separator: &SeparatorStyle,
        pixels_per_point: f32,
    ) -> Option<Rect> {
        let node = &self.dock_state[path.surface][path.node];
        let split = node.get_split()?;
        let fraction = split.fraction;
        let horizontal = node.is_horizontal();

        if !horizontal {
            let [left, right] = self.child_paths(path);
            if self.dock_state[left].is_collapsed() || self.dock_state[right].is_collapsed() {
                return None;
            }
        }

        let rect = split_rect(self.layout.rect(path)?, pixels_per_point);
        let (near, range) = if horizontal {
            (rect.min.x, rect.width())
        } else {
            (rect.min.y, rect.height())
        };
        let midpoint = SeparatorBand::new(fraction, range, separator.extra).midpoint(near, range);
        let low = map_to_pixel(
            midpoint - separator.width * 0.5,
            pixels_per_point,
            f32::round,
        );
        let high = map_to_pixel(
            midpoint + separator.width * 0.5,
            pixels_per_point,
            f32::round,
        );

        let mut drawn = rect;
        if horizontal {
            drawn.min.x = low;
            drawn.max.x = high;
        } else {
            drawn.min.y = low;
            drawn.max.y = high;
        }
        Some(drawn)
    }

    fn show_separator(
        &mut self,
        ui: &mut Ui,
        path: NodePath,
        fade_style: Option<&Style>,
        state: &mut State,
    ) {
        assert!(self.dock_state[path.surface][path.node].is_parent());

        // If either of the children is collapsed, we don't want the user to interact with the separator
        let [left_path, right_path] = self.child_paths(path);
        if (self.dock_state[left_path].is_collapsed() || self.dock_state[right_path].is_collapsed())
            && self.dock_state[path].is_vertical()
        {
            return;
        }

        // Cloned out of `style` up front, and not where they are used: `style` may be borrowed
        // from `self.style`, while everything below that *writes* — `nudge_split`, the junction
        // handles — takes `&mut self`. Holding the borrow across those calls is what used to
        // force the write to be inlined here, in a `&mut` match on the node, where no second
        // caller could reach it.
        let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
        let separator_style = style.separator.clone();
        let toggle_style = style.cross_split_toggle.clone();
        let pixels_per_point = ui.ctx().pixels_per_point();
        // The frame this pass is, which is how a gesture in the field is told alive from stale —
        // see `DragInFlight::pass`.
        let pass = ui.ctx().cumulative_pass_nr();

        // Where the divider is *this frame* — one derivation, shared with everything else that
        // needs to know (see `separator_rect`). The collapsed cases it answers `None` for have
        // already returned above.
        let drawn = self
            .separator_rect(path, &separator_style, pixels_per_point)
            .expect("a separator is only drawn for a split the layout pass just cut");

        duplicate! {
            [
                orientation   dim_point;
                [Horizontal]  [x];
                [Vertical]    [y];
            ]
            if let Node::orientation(_) = &self.dock_state[path.surface][path.node] {
                // Which axis this split divides, and nothing else is read off the node: the
                // borrow of the tree ends here, because every write below goes through
                // `nudge_split`, which takes `&mut self`. What the gesture answers to — the band
                // this frame's geometry can honour — lives there too, so the divider drawn, the
                // rectangle it is grabbed by and the ratio a drag writes all name one line
                // (see `SeparatorBand`).
                let separator = drawn;

                let mut expand = Vec2::ZERO;
                expand.dim_point += separator_style.extra_interact_width / 2.0;
                let interact_rect = separator.expand2(expand);

                let resize_id = ui.id().with((path.node, "separator"));
                let response = ui.interact(interact_rect, resize_id, Sense::click_and_drag())
                    .on_hover_and_drag_cursor(paste!{ CursorIcon::[<Resize orientation>]});

                let should_respond_to_arrow_keys = ui.input(|i| i.modifiers.command || i.modifiers.shift);

                if response.has_focus() {
                    // Prevent the default behaviour of removing focus from the separators when the
                    // arrow keys are pressed
                    ui.memory_mut(|m| m.set_focus_lock_filter(response.id, EventFilter {
                        horizontal_arrows: should_respond_to_arrow_keys,
                        vertical_arrows: should_respond_to_arrow_keys,
                        tab: false,
                        escape: false
                    }));
                }

                let arrow_key_offset = if response.has_focus() && should_respond_to_arrow_keys {
                    if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
                        Some(egui::vec2(0., -16.))
                    } else if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
                        Some(egui::vec2(0., 16.))
                    } else if ui.input(|i| i.key_pressed(Key::ArrowLeft)) {
                        Some(egui::vec2(-16., 0.))
                    } else if ui.input(|i| i.key_pressed(Key::ArrowRight)) {
                        Some(egui::vec2(16., 0.))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let color = if response.dragged() {
                    separator_style.color_dragged
                } else if response.hovered() || response.has_focus() {
                    separator_style.color_hovered
                } else {
                    separator_style.color_idle
                };

                ui.painter().rect_filled(separator, CornerRadius::ZERO, color);

                // Update 'fraction' interaction after drawing separator,
                // otherwise it may overlap on other separator / bodies when
                // shrunk fast.
                //
                // Mouse drag is *continuous*: `drag_delta()` is non-zero on
                // every frame the user holds and moves the separator, and
                // `split.fraction` updates live so the UI tracks the cursor.
                // Emitting `LayoutCommitted` per frame here would force
                // consumers to dedupe an "interaction in progress" stream
                // themselves; we emit `SeparatorDragging` instead and let
                // `drag_stopped()` below produce a single `LayoutCommitted`
                // per completed drag.
                //
                // Arrow-key nudges (`arrow_key_offset.is_some()`) are atomic
                // per keypress, so each one is a finalised event right away.
                //
                // What the hand holds is named once, in the one place that
                // remembers it (`State::begin_drag`), and the commit gate below
                // reads that gesture's `moved` — see `DragInFlight::moved` for
                // what it is for and why it is a flag rather than the starting
                // ratio.
                if response.drag_started() {
                    state.begin_drag(
                        response.id,
                        DragSubject::Separator { path },
                        // egui reports where the press landed on the frame it decides a drag —
                        // it is what `drag_started()` is derived from.
                        response
                            .interact_pointer_pos()
                            .expect("a drag that started was pressed somewhere"),
                        pass,
                    );
                }

                // `drag_delta()` is zero on any frame the separator is not being dragged, so a
                // non-zero delta *is* the gesture; arrow nudges are never zero either. What the
                // delta is allowed to do to the stored ratio — and when it is allowed to do
                // nothing at all — is `nudge_split`'s business, shared with the junction
                // handles, which move two or three of these at once.
                let is_arrow = arrow_key_offset.is_some();
                let delta = arrow_key_offset.unwrap_or(response.drag_delta()).dim_point;
                if self.nudge_split(path, pixels_per_point, separator_style.extra, delta) {
                    if is_arrow {
                        self.events.push(DockEvent::LayoutCommitted);
                    } else {
                        // The drag wrote something, so the gesture holding this divider is the
                        // one that owes a commit on release. A keyboard nudge is not a gesture
                        // and holds nothing — it commits on the spot, above.
                        state.mark_drag_moved();
                        self.events.push(DockEvent::SeparatorDragging);
                    }
                }

                if response.dragged() {
                    // Alive this frame, so a stale entry can be told from a live one — the same
                    // reporting a junction handle does, for the same reason.
                    state.keep_drag_alive(response.id, pass);
                }

                // egui only flips `drag_stopped` after `drag_started`, so a
                // simple click without motion does not reach this branch. What
                // decides the commit is whether the gesture ever moved
                // anything: a grab-and-release with no effective motion would
                // otherwise emit a commit event with no mutation behind it,
                // which breaks snapshot-diffing consumers.
                if response.drag_stopped()
                    && state.end_drag(response.id).is_some_and(|drag| drag.moved)
                {
                    self.events.push(DockEvent::LayoutCommitted);
                }

                if response.double_clicked() && self.split_fraction(path) != 0.5 {
                    self.dock_state[path.surface][path.node]
                        .get_split_mut()
                        .expect("a separator is only drawn for a split")
                        .fraction = 0.5;
                    self.events.push(DockEvent::LayoutCommitted);
                }
            }
        }

        self.draw_junction_handles(ui, path, &separator_style, &toggle_style, state);
    }

    /// The ratio stored on the split at `path`.
    ///
    /// # Panics
    ///
    /// If `path` does not name a split.
    #[track_caller]
    fn split_fraction(&self, path: NodePath) -> f32 {
        self.dock_state[path]
            .get_split()
            .expect("only a split has a fraction")
            .fraction
    }

    /// The interval the split at `path` is cut from this frame, and the band its boundary may
    /// be moved in — `None` wherever a gesture cannot move it at all.
    ///
    /// The `None` cases are the three guards a write has to pass, stated once rather than at
    /// each of the two call sites. A node with no rectangle or no split has no boundary;
    /// `range > 0.0` because `delta / range` is not finite otherwise; and `band.min < band.max`
    /// because on a node too short to leave the margin on both sides the band is the single
    /// point `0.5`, so the clamp answers `0.5` whatever the delta was — writing that "answer"
    /// replaces the stored ratio with dead centre, which is the loss [`SeparatorBand`] exists to
    /// prevent. Found by the frame sweep, on a divider a tab drag grabbed by accident: 0.75
    /// became 0.5 during a step that never named it.
    fn split_gesture(
        &self,
        path: NodePath,
        pixels_per_point: f32,
        extra: f32,
    ) -> Option<(f32, SeparatorBand)> {
        let node = &self.dock_state[path];
        let split = node.get_split()?;
        let rect = split_rect(self.layout.rect(path)?, pixels_per_point);
        let range = if node.is_horizontal() {
            rect.width()
        } else {
            rect.height()
        };
        let band = SeparatorBand::new(split.fraction, range, extra);
        // Negated on purpose, and clippy's rewrite is not equivalent — see `SeparatorBand::new`.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(range > 0.0) || band.min >= band.max {
            return None;
        }
        Some((range, band))
    }

    /// Moves the boundary of the split at `path` by `delta` points along its own axis. Answers
    /// whether the stored ratio changed.
    ///
    /// **The only place a gesture writes a `fraction`** — see the note on [`SeparatorBand`]
    /// listing every writer there is and why that list is worth keeping short. `show_separator`
    /// calls it for the divider under the pointer, a junction handle for the two boundaries a
    /// corner is made of; both get the same clamp because it is the same function, not the same
    /// idea written twice.
    ///
    /// The delta is in **points**, not in fractions, and that is what makes a junction possible:
    /// the boundary is drawn at `near + range * effective`, so `delta / range` moves it by
    /// exactly `delta` points whatever interval it was cut from.
    fn nudge_split(
        &mut self,
        path: NodePath,
        pixels_per_point: f32,
        extra: f32,
        delta: f32,
    ) -> bool {
        if delta == 0.0 {
            return false;
        }
        let Some((range, band)) = self.split_gesture(path, pixels_per_point, extra) else {
            return false;
        };
        let new_fraction = (band.effective + delta / range).clamp(band.min, band.max);
        let split = self.dock_state[path.surface][path.node]
            .get_split_mut()
            .expect("`split_gesture` answered, so the node is a split");
        if split.fraction == new_fraction {
            return false;
        }
        split.fraction = new_fraction;
        true
    }
}

/// The rectangle a split's children are cut out of, snapped out to whole pixels.
///
/// A function, and called by both halves, because they have to name the *same* rectangle:
/// [`DockArea::compute_rect_sizes`] cuts the two children out of it, and
/// [`DockArea::show_separator`] draws the divider between them against it, hit-tests it there
/// and moves it from there. They used to derive it separately — the layout pass snapped, the
/// separator did not — so the divider was measured against a slightly different node than the
/// children were: a boundary up to `2 / pixels_per_point` px off the gap it is supposed to sit
/// in, and a [`SeparatorBand`] computed from a different `range` on top of that. Sub-pixel in
/// every scene we could reach (measured: 0.08 px at `ppp = 1.3`, against 0.17 px of the
/// pixel-rounding both sides share anyway), which is exactly why it would have been found by
/// someone looking at the code rather than at the screen.
/// How tall a strip of `rows` collapsed leaves is: a tab bar each, and a divider between
/// every two of them.
///
/// The dividers are the part that was missing. A collapsed leaf draws a tab bar and nothing
/// else, so `rows * tab_bar.height` is what the *bars* come to — but the leaves are stacked by
/// splits, and every split puts a `separator.width` divider between its two children. Leave the
/// `rows - 1` dividers out and the strip is asked to fit into less than it draws, which is not
/// an error anywhere: the last row is simply cut off by whatever encloses it.
///
/// Zero rows is not a strip and has no height — and no divider count either, which is why the
/// subtraction is guarded rather than written straight out.
pub(super) fn collapsed_strip_height(rows: i32, style: &Style) -> f32 {
    if rows <= 0 {
        return 0.0;
    }
    rows as f32 * style.tab_bar.height + (rows - 1) as f32 * style.separator.width
}

/// How far inside its own rectangle a surface has to start drawing to leave the border it
/// paints there visible.
///
/// The border is stroked `StrokeKind::Inside`, so its full width is inside the rectangle — and
/// if it is rounded, the arc bulges further in than that at each corner. A circle of radius `r`
/// inscribed in the corner leaves the corner point `r - r / sqrt(2)` away from the arc along
/// each axis, and that is the whole of the difference: content inset by this much clears both
/// the stroke and the rounding, at every corner, for any radius a style asks for.
///
/// Inset by half the stroke and nothing else — which is what this used to be — and the first
/// thing drawn paints over the outer half of the border, over the whole of the arc at the
/// corners, and the border the style asked for is simply not there.
///
/// Per side, and not one number for all four, because a rectangle has four radii and a style is
/// free to round one corner and leave the rest square. Each side answers to the two corners it
/// runs between: the top edge is pushed down by the deeper of the north-west and north-east
/// arcs, and knows nothing of the southern two. The distinction is free — the caller was
/// shrinking a rectangle either way — and it is the difference between a rounded title corner
/// costing the layout a strip along one edge and costing it a strip along all four.
fn border_clearance(style: &Style) -> MarginF32 {
    // 1 - 1/sqrt(2), how far a quarter arc bulges in from the corner it is inscribed at, as a
    // fraction of its radius.
    const ARC_BULGE: f32 = 0.292_893_2;

    let radius = style.main_surface_border_rounding;
    let stroke = style.main_surface_border_stroke.width;
    let bulge = |a: u8, b: u8| stroke + f32::from(a.max(b)) * ARC_BULGE;

    MarginF32 {
        left: bulge(radius.nw, radius.sw),
        right: bulge(radius.ne, radius.se),
        top: bulge(radius.nw, radius.ne),
        bottom: bulge(radius.sw, radius.se),
    }
}

fn split_rect(node_rect: Rect, pixels_per_point: f32) -> Rect {
    debug_assert!(!node_rect.any_nan() && node_rect.is_finite());
    expand_to_pixel(node_rect, pixels_per_point)
}

/// The band a split's boundary may occupy this frame, and where the stored ratio sits inside it.
///
/// [`SeparatorStyle::extra`](crate::SeparatorStyle::extra) is a margin in *pixels* that each
/// child must keep, so on a node `range` px long it is the fraction `extra / range`. Two things
/// come out of that, and keeping them apart is the whole reason this type exists:
///
/// * `min` / `max` — the limits a **gesture** may write between;
/// * `effective` — where the boundary is **drawn** and where the children are cut, which is the
///   stored ratio pushed into those limits *without being written back*.
///
/// The separation matters because the band depends on geometry and the ratio does not. Applying
/// the band to `SplitNode::fraction` on every frame — which is what this code used to do, drag or
/// no drag — turns a window resize into a silent edit of the layout: on a node shorter than
/// `2 * extra` the band is the single point `0.5`, so the ratio the user set is replaced by dead
/// centre and growing the window back does not bring it home. A ratio is state; only a gesture
/// gets to change it. Geometry gets to decide where it is honoured.
///
/// # Everything that writes a fraction, and whether it asks
///
/// The clamp is applied when the boundary is *drawn* and is never written back, so a ratio the
/// band cannot hold is not an error anywhere — it is simply drawn somewhere else than it says.
/// That makes every writer a place where the tree and the screen can quietly part company, and
/// there are only four of them:
///
/// * [`DockArea::nudge_split`] — every gesture that moves a boundary, whether the drag and the
///   arrow keys in [`DockArea::show_separator`] or a drag on a junction handle, which moves two
///   or three of them at once. It clamps into `min..max`, so it asks. One function and not one
///   per gesture: a second copy of this arithmetic is a second answer to "how far may this go";
/// * the double-click in the same place — writes `0.5`, which is in every band there is, since
///   `min = (extra / range).min(0.5)` and `max = 1.0 - min`;
/// * [`DockArea::transpose_cross_split`], the one writer that derives a fraction from measured
///   pixels — asks up front, for every fraction the rebuild will write, through
///   `Band::parts_can_be_renested`, and declines the whole gesture rather than move a pixel;
/// * [`DockState::split`] and friends, where the number comes from the caller.
///
/// Nothing derived from geometry escapes unasked today. What keeps that true is not this list —
/// lists go stale — but [`TreeViolation::SplitFractionOutOfRange`](crate::TreeViolation), which
/// catches the arithmetic that answers outside the interval it was measuring, wherever it is
/// written from.
#[derive(Clone, Copy, Debug)]
struct SeparatorBand {
    /// Lowest fraction a gesture may write.
    min: f32,
    /// Highest fraction a gesture may write. Always `1.0 - min`, so always `>= min`.
    max: f32,
    /// The stored fraction as this frame's geometry can honour it: `fraction.clamp(min, max)`.
    effective: f32,
}

impl SeparatorBand {
    /// `range` is the node's extent along the split axis, `extra` is
    /// [`SeparatorStyle::extra`](crate::SeparatorStyle::extra); both in points.
    fn new(fraction: f32, range: f32, extra: f32) -> Self {
        // A node with no extent has no room for a margin and nothing to show either. Answering
        // "no constraint" keeps this a total function of its arguments instead of a special
        // case every caller has to remember; the callers that could act on it guard on `range`
        // anyway, because `delta / range` is not finite here.
        //
        // The negation is load-bearing and clippy's rewrite of it is not equivalent: `range`
        // is `f32`, so a NaN — which is what a degenerate rectangle hands us — answers `false`
        // to *every* comparison. `!(range > 0.0)` is therefore true for NaN and takes this
        // early return, while the suggested `range <= 0.0` is false for it and would carry the
        // NaN into the arithmetic below, where it silently becomes a fraction.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(range > 0.0) {
            return Self {
                min: 0.0,
                max: 1.0,
                effective: fraction,
            };
        }

        // Capping the margin at half the node is what makes an impossible margin degrade
        // sensibly: the band shrinks to a point and the boundary sits at the centre — an equal
        // split, which is the least-bad answer when there is no room to give. The previous
        // normalisation `(min.min(max), max.max(min))` instead *swapped* the inverted pair, so
        // `extra / range >= 1` produced the interval `(0, 1)`: no constraint at all, exactly on
        // the nodes where it was the only thing standing between a child and zero size. Found
        // by the frame harness — a drag on a 175 px node drove `fraction` to 0.0.
        let min = (extra / range).min(0.5);
        let max = 1.0 - min;
        Self {
            min,
            max,
            effective: fraction.clamp(min, max),
        }
    }

    /// Where the boundary falls along the split axis, given the node's near edge and its extent
    /// along that axis — the same `range` this band was built from.
    ///
    /// A node with no extent has no boundary: it is its own edge. That case is here rather than
    /// at the call sites because it is the only place both of them would have had to remember
    /// it, and one of them already got it wrong by omission once.
    fn midpoint(&self, min: f32, range: f32) -> f32 {
        if range > 0.0 {
            min + range * self.effective
        } else {
            min
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SeparatorBand;

    /// A degenerate node hands the band a `range` that is not a positive number, and the two
    /// ways it can fail to be one are **not** the same test: zero is ordinary arithmetic, NaN
    /// answers `false` to every comparison it is put in.
    ///
    /// This pins the `#[allow(clippy::neg_cmp_op_on_partial_ord)]` above `SeparatorBand::new`.
    /// The lint's suggested rewrite — `range <= 0.0` — is false for NaN, so the guard would be
    /// skipped and the NaN would flow into `fraction.clamp(min, max)` and out as a fraction the
    /// tree then stores. Taking the suggestion turns the second half of this test red, which is
    /// the whole reason the `allow` is allowed to stay.
    #[test]
    fn a_range_that_is_not_a_positive_number_constrains_nothing() {
        for range in [0.0, -1.0, f32::NAN] {
            let band = SeparatorBand::new(0.25, range, 4.0);
            assert_eq!(
                (band.min, band.max, band.effective),
                (0.0, 1.0, 0.25),
                "range {range} should have left the fraction alone"
            );
        }
    }

    /// The band is symmetric by construction, and the fraction is what the geometry can honour.
    #[test]
    fn a_margin_too_big_for_the_node_collapses_the_band_to_the_centre() {
        let band = SeparatorBand::new(0.9, 10.0, 40.0);
        assert_eq!(band.min, band.max, "an impossible margin leaves no band");
        assert_eq!(
            band.effective, band.min,
            "the boundary sits where the band is"
        );
        assert!(
            (band.effective - 0.5).abs() < f32::EPSILON,
            "the least-bad answer with no room to give is an equal split, got {}",
            band.effective
        );
    }
}
