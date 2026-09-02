use std::collections::VecDeque;

use egui::{
    Context, CornerRadius, CursorIcon, EventFilter, Key, Pos2, Rect, Sense, StrokeKind, Ui, Vec2,
    epaint::MarginF32,
};

use super::{
    DockAreaResponse, DockMutation,
    drag_and_drop::{DragSource, HoverData, overlay_layer, register_overlay_layer},
    events::DockEvent,
    state::{DragSubject, State},
    tab_removal::TabRemoval,
};
use crate::Share;
use crate::core::resize::{SepBehavior, apply_drag};
use crate::dock_area::tab_removal::ForcedRemoval;
use crate::layout::{DockLayout, SideStrip};
use crate::tab_viewer::OnCloseResponse;
use crate::{
    AllowedSplits, DockArea, Node, OverlayType, Style, SurfaceIndex, TabDestination, TabViewer,
    utils::{expand_to_pixel, fade_dock_style, map_to_pixel},
};
use crate::{GapIndex, GapPath, NodePath, RowGap};

mod junction;
mod leaf;
mod main_surface;
mod modifiers;
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
        self.apply_render_mutations(
            &mutations,
            state.last_hover_pos,
            ui.ctx().pixels_per_point(),
            tab_viewer,
        );

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
        pixels_per_point: f32,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
    ) {
        let mut new_focused = mutations.iter().rev().find_map(|mutation| match mutation {
            DockMutation::Focus(path) => Some(*path),
            DockMutation::Activate(_)
            | DockMutation::SetLeafCollapsed { .. }
            | DockMutation::SetSplitStowed { .. }
            | DockMutation::SetLeafScroll { .. }
            | DockMutation::SetBoundary { .. }
            | DockMutation::SetShares { .. }
            | DockMutation::SetWindowMinimized { .. }
            | DockMutation::WindowShown { .. }
            | DockMutation::TransposeCross { .. }
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
                    let leaf = self.dock_state.leaf_mut(path.node_path()).unwrap();
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
                    self.dock_state[outer.row.surface].transpose_cross(
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
                    let root = self.dock_state[outer.row.surface]
                        .root()
                        .expect("the surface being laid out has a root: `outer` lives in it");
                    let max_rect = self
                        .layout
                        .rect(NodePath::new(outer.row.surface, root))
                        .expect("the root was laid out at the top of this pass");
                    let mut queue = VecDeque::from([outer.row.node]);
                    while let Some(node) = queue.pop_front() {
                        let Some(children) = self.dock_state[outer.row.surface].children(node)
                        else {
                            continue;
                        };
                        // Queued before the cut below, and only so that the borrow of the tree
                        // ends first — the queue is drained in exactly the order it was.
                        queue.extend(children.iter().copied());
                        self.compute_rect_sizes(
                            pixels_per_point,
                            NodePath::new(outer.row.surface, node),
                            max_rect,
                        );
                    }
                }
                DockMutation::SetLeafCollapsed { path, collapsed } => {
                    if self.dock_state[path].is_collapsed() != collapsed {
                        self.dock_state[path.surface].set_leaf_collapsed(path.node, collapsed);
                        // Reads the collapsed flag it has just written, plus this pass's
                        // geometry, to remember the height an expand has to restore.
                        self.window_update_collapsed(path);
                        self.events.push(DockEvent::LayoutCommitted);
                    }
                }
                DockMutation::SetSplitStowed { path, stowed } => {
                    // Asked of `is_stowed`, not of `is_collapsed`: a side whose leaves all
                    // happen to be collapsed is collapsed without being stowed, and answering
                    // the wrong question here would drop the request that puts it away.
                    if self.dock_state[path].is_stowed() != stowed {
                        self.dock_state[path.surface].set_split_stowed(path.node, stowed);
                        // Same reason as for a collapsed leaf: in a floating window this is what
                        // the window's height follows, and stowing changes whether the root of
                        // that window is collapsed.
                        self.window_update_collapsed(path);
                        self.events.push(DockEvent::LayoutCommitted);
                    }
                }
                DockMutation::SetLeafScroll { path, scroll } => {
                    self.dock_state
                        .leaf_mut(path)
                        .expect("a scroll is only requested for a leaf")
                        .scroll = scroll;
                    // No `LayoutCommitted`: scrolling a tab bar has never been a layout edit a
                    // consumer diffs, and it is requested on plain resizes too (the clamp).
                }
                DockMutation::SetBoundary { gap, at } => {
                    self.dock_state[gap.row.surface][gap.row.node]
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
                    self.dock_state[row.surface][row.node]
                        .get_row_mut()
                        .expect("weights are only requested for a row")
                        .set_shares(shares.clone());
                    // No event, for the same reason `SetBoundary` pushes none: the gesture that
                    // asked has already said what it was.
                }
                DockMutation::SetWindowMinimized { surface, minimized } => {
                    // Pushes `LayoutCommitted` itself, as it did when it ran during the click.
                    self.window_set_minimized(surface, minimized);
                }
                DockMutation::WindowShown {
                    surface,
                    took_expanded_height,
                } => {
                    self.dock_state
                        .get_window_state_mut(surface)
                        .expect("the window was drawn this frame")
                        .requests_honoured(took_expanded_height);
                }
                DockMutation::Remove(_) | DockMutation::Detach(_) | DockMutation::Focus(_) => (),
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
            let Node::Leaf(leaf) = &self.dock_state[path.node_path()] else {
                unreachable!("tab drags can only come from leaf nodes")
            };
            tab_viewer.allowed_in_windows(&leaf[path.tab])
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

        // What is inside a side that was put away: not on screen this frame, at any depth.
        // Asked of the tree once, because it is a question about the tree — and answered
        // before the layout runs rather than during it, so that the two later passes read the
        // same set as the first.
        let hidden = self.dock_state[surf_index].stowed_away();

        // First compute all rect sizes in the node graph.
        let pixels_per_point = ui.ctx().pixels_per_point();
        let max_rect = self.allocate_area_for_root_node(ui, surf_index);
        for node in order.iter().copied() {
            let path = NodePath::new(surf_index, node);
            if hidden.contains(&node) {
                // The one thing the pass does with a hidden node, and it is not "nothing":
                // last frame's rectangle has to go, or drawing — which asks the layout instead
                // of deciding for itself — will keep finding the subtree where it used to be.
                self.layout.forget(path);
            } else if self.dock_state[path].is_parent() {
                self.compute_rect_sizes(pixels_per_point, path, max_rect);
            }
        }

        // Then, draw the bodies of each leaves — and the bar of each side that was put away,
        // which is the one thing drawn for a node that is not a leaf. Its subtree was left off
        // the map above, so the bar is all there is of it this frame.
        for node in order.iter().copied() {
            let path = NodePath::new(surf_index, node);
            if self.dock_state[path].is_leaf() {
                self.show_leaf(ui, state, path, tab_viewer, fade_style);
            } else if self.dock_state[path].is_stowed() {
                self.show_stowed_split(ui, path, tab_viewer, fade_style.map(|(style, _)| style));
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

    /// The paths of a row's children, in the row's order: first (left / top) to last
    /// (right / bottom).
    ///
    /// A list, because everything downstream of it walks one: [`Self::cut_row`] cuts as many
    /// children as the row holds, [`Self::strip_columns`] folds over them, and
    /// [`Self::compute_rect_sizes`] writes one rectangle per child and one divider per gap.
    /// Nothing here needs the row to be a pair any more, which is what stage 6 of
    /// `docs/PLAN_a_row_holds_many_panels.md` was for.
    ///
    /// # Panics
    ///
    /// If `path` does not name a row.
    #[track_caller]
    fn child_paths(&self, path: NodePath) -> Vec<NodePath> {
        self.dock_state[path]
            .get_row()
            .expect("only a row has children")
            .children()
            .iter()
            .map(|&child| NodePath::new(path.surface, child))
            .collect()
    }

    /// The paths of the two children `gap` lies between, first (left / top) then second.
    ///
    /// Always exactly two, whatever the row holds — that is what a gap *is* — which is why the
    /// junction detector speaks of these rather than of [`Self::child_paths`]: the line a junction
    /// sits on is the line between two neighbours, not "the line of the split".
    ///
    /// # Panics
    ///
    /// If `gap` does not name a gap of a row.
    #[track_caller]
    fn gap_neighbours(&self, gap: GapPath) -> [NodePath; 2] {
        let row = self.dock_state[gap.row]
            .get_row()
            .expect("only a row has gaps");
        let children = row.children();
        assert!(
            row.has_gap(gap.gap),
            "gap {} of a row with {} gaps",
            gap.gap.0,
            row.gap_count()
        );
        [
            NodePath::new(gap.row.surface, children[gap.gap.0]),
            NodePath::new(gap.row.surface, children[gap.gap.0 + 1]),
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

    /// Write the rectangles of `path`'s children into [`Self::layout`], cutting them out of
    /// the rectangle already recorded for `path` itself — along with *where the row was cut*,
    /// one divider per gap, which is what everything downstream reads instead of working it
    /// out again.
    ///
    /// Takes `pixels_per_point` rather than a [`Ui`] because it is also called from
    /// [`Self::transpose_cross_split`], which edits the tree in the middle of a pass and has
    /// to bring the geometry map back in step with the new shape right there — see the note
    /// on staleness in that function.
    fn compute_rect_sizes(&mut self, pixels_per_point: f32, path: NodePath, max_rect: Rect) {
        // A side put away as a unit is one bar for whatever it contains: there are no children
        // to cut it into, and therefore no line between any of them. Said out loud rather than
        // left to the branch not running, because "no divider" is an answer that has to
        // *arrive* — the entry outlives the frame, so a row that stops answering keeps the line
        // it drew before it was stowed, lying across the strip it has become.
        if self.dock_state[path].is_stowed() {
            self.layout.forget_dividers(path);
            return;
        }

        let children = self.child_paths(path);
        // Collected before the writes below, because the row is borrowed from the tree and the
        // map is written through `&mut self`. The order is the row's own, first gap first.
        let gaps: Vec<GapIndex> = self.dock_state[path]
            .get_row()
            .expect("a parent is a row")
            .gaps()
            .collect();
        let cut = self.cut_row(pixels_per_point, path, max_rect);
        // A branch that answered for a different number of children than the row holds has
        // not answered at all. Loud here, once, rather than a silent `zip` that would truncate
        // to the shorter side and leave last frame's rectangle on whoever was left over.
        assert_eq!(
            cut.children.len(),
            children.len(),
            "the cut answered for a row of a different length"
        );
        assert_eq!(cut.dividers.len(), gaps.len(), "one divider per gap");
        assert_eq!(cut.side_strips.len(), children.len(), "one mark per child");

        for (&child, &rect) in children.iter().zip(&cut.children) {
            self.layout.set_rect(child, rect);
        }
        // Unconditional, every gap: a branch cannot leave a stale answer behind by saying
        // nothing, because `RowCut` made it say something for each of them.
        for (gap, divider) in gaps.into_iter().zip(cut.dividers) {
            self.layout.set_divider(GapPath::new(path, gap), divider);
        }
        for (child, side) in children.into_iter().zip(cut.side_strips) {
            if let Some(side) = side {
                self.layout.set_side_strip(child, side);
            }
        }
    }

    /// How many strips wide this child is, being collapsed — or [`None`] if it does not fit in
    /// strips at all.
    ///
    /// One question rather than the two it used to be ("does it fit?" and, elsewhere, "how
    /// wide?"), because the two are one fact and were free to disagree: the width was a
    /// constant, which is the same as answering "one" to the second question wherever the first
    /// said yes. That was true only while a *row* of strips was impossible, and it was
    /// impossible only because this function said so.
    ///
    /// What counts as one strip: a **leaf**, because a collapsed leaf *is* a single tab bar, and
    /// a split that was **stowed** — put away as a unit, which draws one bar for whatever it
    /// contains (see [`SplitNode::stowed`](crate::SplitNode::stowed)).
    ///
    /// What counts as several: a **horizontal** split whose children are all strips. Side by
    /// side is how strips stack, so a row of three collapsed leaves is three strips wide and
    /// belongs against the edge like any one of them. This is the case the pair-shaped rule
    /// could not express — a binary tree writes that row as `H(a, H(b, c))`, and the inner
    /// split's two collapsed children each blocked the other, because "my sibling is open" was
    /// how "somebody can take the width" was spelled. Somebody could: the open column further
    /// along the row.
    ///
    /// What does not fit at all is a **vertical** split collapsed one leaf at a time: that is
    /// rows of tab bars stacked downwards, and rows do not fit in something one tab bar wide.
    /// Note that this is *not* the same distinction as stowing, which remains state of its own:
    /// a stowed split is one bar for its whole subtree whichever way it is split, and a
    /// horizontal split collapsed leaf-by-leaf is as many bars as it has leaves.
    fn strip_columns(&self, path: NodePath) -> Option<i32> {
        let node = &self.dock_state[path];
        // Not collapsed, nothing to squeeze. Asked first so the recursion below cannot be
        // reached by an open subtree through a collapsed ancestor.
        if !node.is_collapsed() {
            return None;
        }
        if node.is_leaf() || node.is_stowed() {
            return Some(1);
        }
        if !node.is_horizontal() {
            return None;
        }
        // A fold, not a pair: a row of strips is as many strips wide as its children add up to,
        // and one child that does not fit is a row that does not fit — which is what summing
        // `Option`s says, `None` absorbing the whole sum.
        self.child_paths(path)
            .into_iter()
            .map(|child| self.strip_columns(child))
            .sum()
    }

    /// Whether this child draws **one** bar of its own when it becomes a strip, which is what
    /// decides whether the layout marks *it* as the strip or leaves that to its children.
    ///
    /// A leaf and a stowed split do; a horizontal split collapsed leaf-by-leaf does not — it is
    /// a row of strips, one per leaf, and each of those is marked when the pass reaches it. Get
    /// this wrong the generous way and the row draws one arrow for the whole thing while its
    /// leaves keep their own underneath.
    fn draws_one_bar(&self, path: NodePath) -> bool {
        self.dock_state[path].is_leaf() || self.dock_state[path].is_stowed()
    }

    /// How long each child of the row at `path` is *asked* to be this frame — a fixed length for
    /// a collapsed child squeezed into a strip, a weight for everything else — or [`None`] where
    /// every child takes a share and the row is simply cut at its stored boundaries.
    ///
    /// **One home for "which children are strips this frame".** Two readers depend on the
    /// answer: [`cut_row`](Self::cut_row) turns it into rectangles, and
    /// [`trading_pair`](Self::trading_pair) turns it into "which two children does a drag on
    /// this divider actually trade between". Written out twice — the layout deciding and the
    /// gesture guessing from the tree — they would be free to disagree, which is the shape of
    /// the bug this file already records once (the "is there a divider?" rule that lived inline
    /// in two places and heard about the sideways branch in neither).
    ///
    /// The two cases are the same problem one axis over, and are kept apart only by what they
    /// measure: a collapsed child of a **vertical** row spends height ([`collapsed_strip_height`],
    /// as many rows as it has collapsed leaves), a strip child of a **horizontal** row spends
    /// width ([`collapsed_strip_width`], as many columns as [`strip_columns`](Self::strip_columns)
    /// says it fits in). Sideways is behind [`DockArea::collapse_sideways`]; collapsing into a row
    /// is not, and the reason for the asymmetry is in `cut_row`.
    fn row_extents(&self, path: NodePath) -> Option<RowExtents> {
        let row = self.dock_state[path]
            .get_row()
            .expect("only a row has children to measure");
        let horizontal = row.is_horizontal();
        let style = self.style.as_ref().unwrap();
        let children = self.child_paths(path);
        let weight = |index: usize| Extent::Weighted(row.shares()[index].0);

        if !horizontal {
            if !children
                .iter()
                .any(|&child| self.dock_state[child].is_collapsed())
            {
                return None;
            }
            let extents = children
                .iter()
                .enumerate()
                .map(|(index, &child)| {
                    let node = &self.dock_state[child];
                    if node.is_collapsed() {
                        Extent::Fixed(collapsed_strip_height(node.collapsed_leaf_count(), style))
                    } else {
                        weight(index)
                    }
                })
                .collect();
            return Some(RowExtents {
                extents,
                sideways: false,
            });
        }

        if !self.collapse_sideways {
            return None;
        }
        let columns: Vec<Option<i32>> = children
            .iter()
            .map(|&child| self.strip_columns(child))
            .collect();
        if !columns.iter().any(Option::is_some) {
            return None;
        }
        let extents = columns
            .iter()
            .enumerate()
            .map(|(index, columns)| match columns {
                Some(columns) => Extent::Fixed(collapsed_strip_width(*columns, style)),
                None => weight(index),
            })
            .collect();
        Some(RowExtents {
            extents,
            sideways: true,
        })
    }

    /// The two children a drag on the divider in `gap` actually trades between: the nearest child
    /// on either side that takes a *share* of the row.
    ///
    /// On a row with no strips in it those are the gap's own two neighbours, `(gap, gap + 1)`, and
    /// the caller can tell that case by the pair being adjacent — it is the one that must keep
    /// writing exactly the bits it always wrote.
    ///
    /// With a strip between them they are the open children on either side of it. Both of the
    /// strip's edges answer the same pair, which is what makes the two lines
    /// [`cut_runs`](cut_runs) draws there one gesture with two handles rather than two gestures:
    /// the strip keeps its own width whatever the drag does, so the only thing a hand can move
    /// here is how the two open columns divide what is left, and the strip slides along with the
    /// line.
    ///
    /// [`None`] when one side is nothing but strips — a strip stacked against the row's end has
    /// no second party to trade with, and no divider is drawn beside it either.
    fn trading_pair(&self, gap: GapPath) -> Option<(usize, usize)> {
        let row = self.dock_state[gap.row].get_row()?;
        row.has_gap(gap.gap).then_some(())?;
        let Some(RowExtents { extents, .. }) = self.row_extents(gap.row) else {
            return Some((gap.gap.0, gap.gap.0 + 1));
        };
        let is_open = |index: usize| matches!(extents[index], Extent::Weighted(_));
        let near = (0..=gap.gap.0).rev().find(|&index| is_open(index))?;
        let far = (gap.gap.0 + 1..extents.len()).find(|&index| is_open(index))?;
        Some((near, far))
    }

    /// How the row at `path` is cut this frame: one rectangle per child, one divider per gap
    /// (where there is one to draw), and which children became sideways strips.
    ///
    /// One value with three fields rather than three branches that each write what they
    /// remember to: adding a fourth way to cut a row now means filling this in, and the
    /// compiler says so. The bug that motivated the shape was exactly the other arrangement —
    /// a branch was added here, and the "is there a divider?" rule, written out separately in
    /// the code that draws, kept answering for the branches that existed when it was written.
    ///
    /// Three branches, each written over `n` children — stage 6 of
    /// `docs/PLAN_a_row_holds_many_panels.md`, the last parity stage: on a pair every branch
    /// below cuts the pixels it cut when it was written over `left` / `right`, and the corpus
    /// probes (`rect_probe`, `shape_probe`) say so byte for byte. Two of the three share their
    /// arithmetic, [`cut_runs`]: the collapsed children of a **vertical** row and the strip
    /// children of a **horizontal** one are the same shape one axis over — fixed lengths pressed
    /// against the row's edges, with the open children sharing what is left — and the
    /// pair-shaped code had solved that twice, which is how the 30.08 bug lived in one axis and
    /// not the other.
    fn cut_row(&self, pixels_per_point: f32, path: NodePath, max_rect: Rect) -> RowCut {
        let row = self.dock_state[path]
            .get_row()
            .expect("only a row is cut into children");
        let horizontal = row.is_horizontal();
        let style = self.style.as_ref().unwrap();
        let children = self.child_paths(path);

        // The parent's rectangle was written either by `allocate_area_for_root_node` (for
        // the root) or by this same function when its own parent was visited — the
        // breadth-first order of the caller guarantees it is already there.
        let parent_rect = self
            .layout
            .rect(path)
            .expect("a parent node must have been laid out before its children");
        let rect = split_rect(parent_rect, pixels_per_point);

        // The row's axis as a *value*, read once: nothing below names a method by its axis, so
        // one body serves both — the same shape `show_divider` was rewritten into once the axis
        // stopped being the variant of the node.
        let (lo, hi, size) = if horizontal {
            (rect.min.x, rect.max.x, rect.width())
        } else {
            (rect.min.y, rect.max.y, rect.height())
        };
        let after = |at: f32| {
            if horizontal {
                Rect::everything_right_of(at)
            } else {
                Rect::everything_below(at)
            }
        };
        let before = |at: f32| {
            if horizontal {
                Rect::everything_left_of(at)
            } else {
                Rect::everything_above(at)
            }
        };
        // A child's rectangle from where it was cut. `None` is the row's own edge: the first
        // child starts where the row starts and the last one ends where it ends, without a cut
        // being snapped onto an edge that is on the pixel grid already.
        let child_rect = |(start, end): Span| {
            let mut child = rect;
            if let Some(start) = start {
                child = child.intersect(after(start));
            }
            if let Some(end) = end {
                child = child.intersect(before(end));
            }
            child.intersect(max_rect)
        };
        // The line between two edges just computed, across the whole of the row the other way.
        // This is the *only* derivation of it in the crate: it used to be worked out here for
        // the children's sake, thrown away, and then worked out a second time by
        // `separator_rect` for drawing — two copies of one arithmetic, which had drifted once.
        let divider_rect = |(near, far): (f32, f32)| {
            let mut divider = rect;
            if horizontal {
                divider.min.x = near;
                divider.max.x = far;
            } else {
                divider.min.y = near;
                divider.max.y = far;
            }
            divider
        };
        let cut_at = |at: f32| map_to_pixel(at, pixels_per_point, f32::round);

        // A vertical row with a collapsed child. The collapsed child is not cut at a ratio — it
        // is given exactly what its rows need, and the divider goes *beside* that, not through
        // it. It used to straddle the boundary, taking half its width out of the collapsed
        // rows: with the boundary at `rows * tab_bar.height` the last row was drawn a hairline
        // taller than the space it had, and the whole strip was one divider short per row.
        //
        // Over `n` — see `cut_runs`: collapsed children at the top of the row are stacked down
        // from its top edge, collapsed children at the bottom are stacked up from its bottom
        // edge, and whatever is open in between shares the rest. With nothing open at all the
        // stack hangs from the top and the **last child keeps the remainder** — the rule the
        // pair had ("either only the first collapsed or both: the strip is the top of the
        // node"). It matters only where the row is taller than its rows, i.e. at the root or
        // inside a column: a collapsed leaf draws its bar at the top of whatever it is given, so
        // the picture is the same either way, and the difference is who *owns* the empty space
        // below. Kept by parity. The horizontal branch answers "nobody" to the same question
        // (Стас, 30.08), and reconciling the two is a decision for stage 7, not a refactor.
        if let Some(RowExtents { extents, sideways }) = self.row_extents(path) {
            // Each edge snapped from the *unsnapped* run — `far = near + width` in points, then
            // to the pixel — the way the pair's `right_separator_border` was. The horizontal
            // branch below snaps the run itself; `cut_runs` says why the two are kept apart.
            // `false`, and this is the one place stage 7 **changed a pixel on purpose**. A fully
            // collapsed vertical row used to let its last child keep the rest of the column,
            // where the horizontal one left the rest to nobody: the same picture — every
            // collapsed leaf draws its bar at the top of whatever it is given — answering
            // differently to a hit test and to a drop target, and letting the thing called a
            // strip quietly not be one. Reconciled on the horizontal answer (decision 7, Стас).
            //
            // The sideways case snaps the run itself instead (`right_start = cut_at(left_end +
            // separator)` with `left_end` already snapped), as the pair did — `cut_runs` says why
            // the two are kept apart rather than unified.
            let runs = cut_runs(
                lo,
                hi,
                &extents,
                style.separator.width,
                cut_at,
                |at| if sideways { cut_at(at) } else { at },
                false,
            );
            // Which children became strips that draw a bar of their own, and against which edge.
            // Only the sideways case marks anything: a child collapsed into a *row* draws a tab
            // bar, which is what a leaf draws anyway, so there is nothing for drawing to be told.
            let side_strips = if sideways {
                children
                    .iter()
                    .zip(&runs.runs)
                    .zip(&extents)
                    .map(|((&child, run), extent)| {
                        // A child that is a row of strips is not marked as one itself: its
                        // leaves are, when the pass reaches them. See `draws_one_bar`.
                        if !matches!(extent, Extent::Fixed(_)) || !self.draws_one_bar(child) {
                            return None;
                        }
                        // `Left` means "the width it gave up lies to its right", which is what
                        // the pair wrote for *both* strips of a fully collapsed row — the second
                        // hugs the first, not the edge. A strip among open columns reads the
                        // same way; only a strip stacked from the far edge is a `Right`.
                        Some(match run {
                            Run::Leading | Run::Middle => SideStrip::Left,
                            Run::Trailing => SideStrip::Right,
                        })
                    })
                    .collect()
            } else {
                vec![None; children.len()]
            };
            return RowCut {
                children: runs.spans.into_iter().map(child_rect).collect(),
                // Cut at the strips' edges, not at the ratios — so a line arrives only where two
                // open children have nothing but strips between them, and it arrives at both
                // edges of what is between them. See `cut_runs`, which decides it, and
                // `trading_pair`, which reads the same extents back to say what such a line
                // moves. A pair with a collapsed child has one open child and gets no line at all.
                dividers: runs
                    .dividers
                    .into_iter()
                    .map(|divider| divider.map(divider_rect))
                    .collect(),
                side_strips,
            };
        }

        // The mirror of the case above, one axis over: a leaf collapsed *sideways* gives up
        // its width instead of its height, and the sibling column takes it.
        //
        // Why this is opt-in and the case above is not: collapsing spends height, so under a
        // vertical split the sibling above or below simply grows into it. Under a horizontal
        // one it cannot — the space freed is a column, and a leaf shrunk to a bar would leave
        // an area with no tab bar, no body and no owner. That is why a collapsed leaf beside a
        // column keeps its column by default (`a_collapsed_leaf_is_one_row.rs` pins it). This
        // path is the other answer to the same problem: spend *width*, which the sibling
        // column can take, so there is nothing left over to belong to nobody.
        //
        // Two conditions, and each one is what keeps the hole from coming back:
        //
        // * only something that **fits in strips** — see `strip_columns`;
        // * only with the knob on, because this reverses a decision users' layouts already
        //   depend on.
        //
        // "Only when the sibling is open" used to be a third, and it was the bug: two collapsed
        // siblings each read the other as "nobody to take the width" and both stayed columns,
        // although in `H(a, H(b, c))` — how a binary tree writes a row of three — the open
        // column was one level out, holding the width for both. Now each strip is given exactly
        // what it needs and whatever is left over is the open children's; when *every* child is
        // a strip there is nothing left to give away, and the leftover belongs to nobody by
        // decision (Стас, 30.08.2026: strips for everyone, the rest empty), which is the one
        // place in this feature where a hole is the answer rather than the thing to avoid. The
        // strips then sit against the near edge one after the other: pressing the last against
        // the *far* edge instead would leave the hole in the middle of the row, the same amount
        // of empty space arranged so that it separates the strips rather than standing beside
        // them.
        // Both of those cases are `row_extents` above, which is the one place that decides them —
        // the gesture reads the same answer through `trading_pair`.

        // Neither: every child is cut at where its boundaries *are*, which is the stored ratio
        // pushed into the band this frame's geometry can honour — see `SeparatorBand`.
        // Clamping here, rather than writing the clamped number back into the tree, is what
        // lets a node with no room for the margin keep the ratio it will get back as soon as
        // there is room again.
        //
        // Each boundary is clamped on its own, exactly as the pair's one boundary was. The
        // boundaries of a row are monotone — running sums of weights `validate` keeps
        // non-negative — and the clamp is monotone, so their order survives.
        //
        // What the band does *not* promise on a row of three is that two boundaries stay a
        // divider apart: two of them can coincide, and then the child between them is asked to
        // fit into the width of a divider *backwards*. That is what the second pass is for.
        // Stage 6 wrote this branch and recorded the hole as unreachable while rows were pairs;
        // stage 7 made it reachable and the corpus found it — 86 inside-out rectangles across
        // 544 layouts, all of them children of long chains flattened on load.
        //
        // A squeezed child is given **nothing**, not less than nothing. A minimum size per child
        // would be the other answer and is deliberately not this feature (`shares` is the shape
        // that admits it later); an inverted rectangle is not an answer at all — it is a panel
        // whose hit test and clipping disagree about which side of itself it is on.
        let mut edges: Vec<(f32, f32)> = row
            .gaps()
            .map(|gap| {
                let band = SeparatorBand::new(row.boundary(gap), size, style.separator.extra);
                let midpoint = band.midpoint(lo, size);
                (
                    cut_at(midpoint - style.separator.width * 0.5),
                    cut_at(midpoint + style.separator.width * 0.5),
                )
            })
            .collect();
        // One pass, forwards, because a divider pushed along by its predecessor pushes the next
        // one in turn. Each edge is put inside the row and never before the edge behind it, so
        // the dividers come out in order and within the row, and a child between two of them is
        // given a span of **zero** rather than of minus a divider.
        //
        // A pair has one divider, no predecessor, and a boundary the band already keeps inside
        // the row, so this cannot touch it: parity.
        //
        // Written first as two passes, one from each end, on the reasoning that the forward one
        // could walk the last divider past `hi`. The second could not be made to fire — not by
        // the corpus (0 inside-out either way) and not by a scene built to aim at it — so it is
        // a sentence here instead of code, and the clamp against `hi` that it existed for is
        // folded into the one pass that is judged.
        let mut floor = lo;
        for edge in &mut edges {
            edge.0 = edge.0.clamp(floor, hi);
            edge.1 = edge.1.clamp(edge.0, hi);
            floor = edge.1;
        }
        // Child `k` runs from the far edge of divider `k − 1` to the near edge of divider `k`;
        // the first and the last reach the row's own edges.
        let spans = (0..children.len()).map(|index| {
            (
                index.checked_sub(1).map(|gap| edges[gap].1),
                (index < edges.len()).then(|| edges[index].0),
            )
        });
        RowCut {
            children: spans.map(child_rect).collect(),
            dividers: edges.iter().map(|&edge| Some(divider_rect(edge))).collect(),
            side_strips: vec![None; children.len()],
        }
    }

    /// The rectangle the divider in `gap` is drawn in — and, expanded by
    /// [`SeparatorStyle::extra_interact_width`](crate::SeparatorStyle::extra_interact_width),
    /// grabbed by.
    ///
    /// `None` where there is no divider on screen to speak of: a gap of nothing (a leaf, or a
    /// gap past the row's last), a row the layout pass has no rectangle for, or a gap it cut at
    /// a strip's edge rather than at its ratio. See [`DockLayout::divider`] for that last one.
    ///
    /// A lookup rather than a derivation, and that is the whole point of the map. The painted
    /// divider, the rectangle it is grabbed by, the cross-split button sized by how close the
    /// nearest other divider is (`DockArea::handle_room`) and the sweep in `tests/dst.rs` all
    /// have to name the same line, and the way to make that true is for one place to decide
    /// where it is. That place is the layout pass, which cannot avoid deciding: cutting the two
    /// neighbours *is* choosing the line between them. It used to compute the line, throw it
    /// away, and leave this function to compute it again from the ratio — along with a separate
    /// rule for whether there was one at all, which is the copy that drifted when the sideways
    /// branch was added.
    ///
    /// It also answers quietly for a gap that names nothing, where indexing the tree here used
    /// to panic (`no node 0.1 in this tree`, from a leaf closed mid-gesture at DST seed 1).
    pub(super) fn separator_rect(&self, gap: GapPath) -> Option<Rect> {
        self.layout.divider(gap)
    }

    /// Draws and interacts every divider of the row at `path`: one per gap.
    ///
    /// The loop is the whole of this function, and it is here rather than in the caller so that
    /// "a row draws its dividers" is said once: a pair has one gap, a row of five has four, and
    /// each is a separator of its own with its own widget, its own gesture and its own junctions.
    fn show_separator(
        &mut self,
        ui: &mut Ui,
        path: NodePath,
        fade_style: Option<&Style>,
        state: &mut State,
    ) {
        // Collected first, because drawing a divider takes `&mut self` and the row is borrowed
        // from the tree. The order is the row's own, first gap first.
        let gaps: Vec<GapIndex> = self.dock_state[path]
            .get_row()
            .expect("only a row has dividers")
            .gaps()
            .collect();
        for gap in gaps {
            self.show_divider(ui, GapPath::new(path, gap), fade_style, state);
        }
    }

    /// One divider: painted where the layout cut it, grabbed there, and moved by the drag, the
    /// arrow keys and the double-click. The junction handles on its line come at the end.
    fn show_divider(
        &mut self,
        ui: &mut Ui,
        gap: GapPath,
        fade_style: Option<&Style>,
        state: &mut State,
    ) {
        let path = gap.row;
        assert!(self.dock_state[path.surface][path.node].is_parent());

        // Where the divider is *this frame*, as the layout pass recorded it — `None` when it
        // cut the gap at a strip's edge instead of at its ratio, and then there is nothing
        // here to paint or to hit-test. Asking the geometry rather than re-deriving the rule is
        // what keeps this from falling behind the next branch added to the layout.
        let Some(drawn) = self.separator_rect(gap) else {
            return;
        };

        // Whether this line has a strip between the two children it trades between — see
        // `trading_pair`. Two things read it: the double-click below, which has no answer for
        // such a line, and the junction handles at the end, which have none either. A junction
        // is a corner where this line meets a divider *inside* one of its neighbours, and it
        // moves both by writing the boundary of this gap — which beside a strip is the boundary
        // of the strip itself, and moves nothing.
        let spans_a_strip = self
            .trading_pair(gap)
            .is_some_and(|(near, far)| far > near + 1);

        // Cloned out of `style` up front, and not where they are used: `style` may be borrowed
        // from `self.style`, while everything below that *writes* — `nudge_boundary`, the
        // junction handles — takes `&mut self`. Holding the borrow across those calls is what
        // used to force the write to be inlined here, in a `&mut` match on the node, where no
        // second caller could reach it.
        let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
        let separator_style = style.separator.clone();
        let toggle_style = style.cross_split_toggle.clone();
        let pixels_per_point = ui.ctx().pixels_per_point();
        // The frame this pass is, which is how a gesture in the field is told alive from stale —
        // see `DragInFlight::pass`.
        let pass = ui.ctx().cumulative_pass_nr();

        if let Node::Row(row) = &self.dock_state[path.surface][path.node] {
            // Which axis this row divides, and nothing else is read off the node: the
            // borrow of the tree ends here, because every write below goes through
            // `nudge_boundary`, which takes `&mut self`. What the gesture answers to — the
            // band this frame's geometry can honour — lives there too, so the divider drawn,
            // the rectangle it is grabbed by and the ratio a drag writes all name one line
            // (see `SeparatorBand`).
            //
            // One body, both axes. This was a `duplicate!` block compiling the whole of it
            // twice — once per axis — from the days when the axis was the *variant* of the node
            // and could only be matched on. It is a field of the row since 30.08 (see
            // `RowNode::horizontal`), so it is read here like any other value, and the two
            // things that genuinely differ say so where they are used: the cursor, and which
            // component of a `Vec2` runs along the row.
            let horizontal = row.is_horizontal();
            // The component of an offset that runs along the row — `x` where the children sit
            // side by side, `y` where they are stacked. Indexed rather than named, which is
            // what lets one body serve both.
            let along = if horizontal { 0 } else { 1 };

            let separator = drawn;

            let mut expand = Vec2::ZERO;
            expand[along] += separator_style.extra_interact_width / 2.0;
            let interact_rect = separator.expand2(expand);

            // The gap is part of the id: a row of three has two of these widgets, and a
            // press has to know which of them it holds.
            let resize_id = ui.id().with((path.node, gap.gap, "separator"));
            let response = ui
                .interact(interact_rect, resize_id, Sense::click_and_drag())
                .on_hover_and_drag_cursor(if horizontal {
                    CursorIcon::ResizeHorizontal
                } else {
                    CursorIcon::ResizeVertical
                });

            let should_respond_to_arrow_keys = ui.input(|i| i.modifiers.command || i.modifiers.shift);

            if response.has_focus() {
                // Prevent the default behaviour of removing focus from the separators when the
                // arrow keys are pressed
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        response.id,
                        EventFilter {
                            horizontal_arrows: should_respond_to_arrow_keys,
                            vertical_arrows: should_respond_to_arrow_keys,
                            tab: false,
                            escape: false,
                        },
                    )
                });
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
                    DragSubject::Separator { gap },
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
            // nothing at all — is `nudge_boundary`'s business, shared with the junction
            // handles, which move two or three of these at once.
            let is_arrow = arrow_key_offset.is_some();
            let delta = arrow_key_offset.unwrap_or(response.drag_delta())[along];
            // What the hand is holding says who pays for this drag — the divider row of
            // `docs/MODIFIERS.md`. A keyboard nudge is `Pair` whatever is held, and not by
            // taste: taking focus at all costs a modifier (`should_respond_to_arrow_keys`
            // above), so Ctrl+arrow has already spent its Ctrl on steering this divider.
            let behavior = if is_arrow {
                SepBehavior::Pair
            } else {
                SepBehavior::from_modifiers(ui.input(|i| i.modifiers))
            };
            if self.drag_boundary(gap, pixels_per_point, separator_style.extra, delta, &behavior) {
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

            // The middle of the room *this divider* has, which is `0.5` on a pair and is not
            // on anything longer. Written as `0.5` flat until stage 4 of
            // `docs/PLAN_a_drag_chooses_who_pays_for_it.md`, where the sweep — once it could
            // build a row of three — caught a double-click writing a boundary clean past its
            // neighbour: the divider between the second and third panels was sent to the
            // middle of the whole row, which is behind the first one.
            //
            // A line drawn beside a strip is left out: the middle of *its* room is the
            // middle between two boundaries that both lie on the same strip, and writing it
            // moves nothing on screen while editing the width the hidden panel is keeping.
            // What a double-click there should mean — the middle of the two open children's
            // shared room, presumably — is a decision and not a derivation, so it waits for
            // one instead of being guessed at here.
            if response.double_clicked() && !spans_a_strip {
                let centre = self.gap_centre(gap);
                if self.boundary_at(gap) != centre {
                    self.mutations
                        .push(DockMutation::SetBoundary { gap, at: centre });
                    self.events.push(DockEvent::LayoutCommitted);
                }
            }
        }

        if !spans_a_strip {
            self.draw_junction_handles(ui, gap, &separator_style, &toggle_style, state);
        }
    }

    /// Where the boundary in `gap` sits, as its row stores it.
    ///
    /// # Panics
    ///
    /// If `gap` does not name a gap of a row.
    #[track_caller]
    fn boundary_at(&self, gap: GapPath) -> f32 {
        self.dock_state[gap.row]
            .get_row()
            .expect("only a row has a boundary")
            .boundary(gap.gap)
    }

    /// Half way between the boundaries either side of `gap` — where a double-click puts it.
    ///
    /// `0.5` for a pair, whose divider has the whole row to itself, and the midpoint of its own
    /// room for anything longer. See [`RowNode::neighbour_boundaries`].
    ///
    /// # Panics
    ///
    /// If `gap` does not name a gap of a row.
    #[track_caller]
    fn gap_centre(&self, gap: GapPath) -> f32 {
        let (lo, hi) = self.dock_state[gap.row]
            .get_row()
            .expect("only a row has a boundary")
            .neighbour_boundaries(gap.gap);
        0.5 * (lo + hi)
    }

    /// The interval the row of `gap` is cut from this frame, and the band the boundary in that
    /// gap may be moved in — `None` wherever a gesture cannot move it at all.
    ///
    /// The `None` cases are the guards a write has to pass, stated once rather than at each of
    /// the two call sites. A node with no rectangle, no row, or no such gap has no boundary;
    /// `range > 0.0` because `delta / range` is not finite otherwise; and `band.min < band.max`
    /// because on a node too short to leave the margin on both sides the band is the single
    /// point `0.5`, so the clamp answers `0.5` whatever the delta was — writing that "answer"
    /// replaces the stored ratio with dead centre, which is the loss [`SeparatorBand`] exists to
    /// prevent. Found by the frame sweep, on a divider a tab drag grabbed by accident: 0.75
    /// became 0.5 during a step that never named it.
    ///
    /// The band is the *row's* whole interval less the margins, which is right for a pair and
    /// is stage 7's to narrow to the two neighbouring boundaries once a row can hold more.
    /// How long the row of `gap` is along its own axis this frame, or `None` if a gesture cannot
    /// address it at all.
    ///
    /// The `None` cases are the guards every write has to pass, stated once: a node with no
    /// rectangle, no row, or no such gap has no boundary — a gesture can outlive the shape it
    /// grabbed — and a range of zero makes `delta / range` not finite.
    ///
    /// This is also the number a drag converts points into weight with (`RowNode::shares_after_drag`
    /// takes it as the row's extent), and deliberately the *whole* row rather than the part its
    /// weighted children divide: [`SeparatorBand`] already measures its margins as `extra / range`
    /// of the same interval, so the two agree by construction. Measuring one of them against the
    /// children's own total would make a minimum mean two slightly different lengths depending on
    /// which of the two clamps a drag ended up in.
    fn row_extent(&self, gap: GapPath, pixels_per_point: f32) -> Option<f32> {
        let node = &self.dock_state[gap.row];
        let row = node.get_row()?;
        row.has_gap(gap.gap).then_some(())?;
        let rect = split_rect(self.layout.rect(gap.row)?, pixels_per_point);
        let range = if node.is_horizontal() {
            rect.width()
        } else {
            rect.height()
        };
        // Negated on purpose, and clippy's rewrite is not equivalent — see `SeparatorBand::new`.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(range > 0.0) {
            return None;
        }
        Some(range)
    }

    fn boundary_gesture(
        &self,
        gap: GapPath,
        pixels_per_point: f32,
        extra: f32,
    ) -> Option<(f32, SeparatorBand)> {
        let range = self.row_extent(gap, pixels_per_point)?;
        let row = self.dock_state[gap.row].get_row()?;
        // The neighbouring boundaries, or the row's own ends where there is no neighbour: a
        // gesture may move this line up to them and no further, because the room on either side
        // of it is the two children's and writing past a neighbour takes a child's room away
        // through zero. On a pair both are the ends, which is the band this always was.
        let (lo, hi) = row.neighbour_boundaries(gap.gap);
        let band = SeparatorBand::between(row.boundary(gap.gap), lo, hi, range, extra);
        // `band.min < band.max` because on a node too short to leave the margin on both sides the
        // band is the single point `0.5`, so the clamp answers `0.5` whatever the delta was —
        // writing that "answer" replaces the stored ratio with dead centre, which is the loss
        // `SeparatorBand` exists to prevent. Found by the frame sweep, on a divider a tab drag
        // grabbed by accident: 0.75 became 0.5 during a step that never named it.
        if band.min >= band.max {
            return None;
        }
        Some((range, band))
    }

    /// Moves the boundary in `gap` by `delta` points under `behavior` — the whole of what a
    /// divider drag does, whichever key the hand is holding. Answers whether anything changed.
    ///
    /// Two paths, and the split is a decision rather than a shortcut:
    ///
    /// * [`Pair`](SepBehavior::Pair) goes through [`nudge_boundary`](Self::nudge_boundary), which
    ///   writes one boundary through `RowNode::set_boundary`. A pair drag therefore writes the
    ///   same bits it wrote before this feature existed, which is what every earlier stage's
    ///   parity rests on, and it is also what the junction handles and the arrow keys ask for.
    /// * the other modes rewrite the *whole* weight vector, because neither has a single boundary
    ///   to name: a chain travels past the neighbour that ran out, a proportional drag moves
    ///   every boundary of the row at once. Their clamp is `min_size` inside
    ///   [`RowNode::shares_after_drag`](crate::RowNode::shares_after_drag) — travelling past a
    ///   neighbour is the *point* of them, so `SeparatorBand`'s neighbour-to-neighbour band would
    ///   be clamping away the feature.
    fn drag_boundary(
        &mut self,
        gap: GapPath,
        pixels_per_point: f32,
        extra: f32,
        delta: f32,
        behavior: &SepBehavior,
    ) -> bool {
        // A divider drawn beside a strip trades between the two *open* children the strip lies
        // between, and neither of the paths below can name that: one writes a boundary, the other
        // walks the row's whole weight vector including the strip's — whose weight buys nothing,
        // because a strip is given its own width whatever it is holding. Taken first so that both
        // the Pair path and the modes go the same way over such a divider, and so that the two
        // lines drawn either side of one strip are one gesture with two handles.
        if let Some((near, far)) = self.trading_pair(gap)
            && far > near + 1
        {
            return self.drag_across_strips(
                gap,
                near,
                far,
                pixels_per_point,
                extra,
                delta,
                behavior,
            );
        }
        if let SepBehavior::Pair = behavior {
            return self.nudge_boundary(gap, pixels_per_point, extra, delta);
        }
        if delta == 0.0 {
            return false;
        }
        let Some(range) = self.row_extent(gap, pixels_per_point) else {
            return false;
        };
        let row = self.dock_state[gap.row]
            .get_row()
            .expect("`row_extent` answered, so this node is a row");
        let shares = row.shares_after_drag(gap.gap, delta, behavior, range, extra);
        // A drag that asked a row with nothing left to give changes no weight, and reporting it as
        // a write would put a commit event behind a layout that did not move.
        if shares == row.shares() {
            return false;
        }
        self.mutations.push(DockMutation::SetShares {
            row: gap.row,
            shares,
        });
        true
    }

    /// Moves the line drawn beside a strip by `delta` points: the two open children `near` and
    /// `far` divide what the strips between them leave, and the strips ride along at their own
    /// width. Answers whether any weight changed.
    ///
    /// **The strips take no part in the trade.** A strip's weight is stored and ignored — the
    /// layout gives it exactly `collapsed_strip_width` — so the row's weight vector is not the
    /// vector this drag divides. The one it divides is the open children's, which is why the
    /// weights are compacted, dragged, and scattered back: whatever a strip's weight was, it is
    /// what it will be when the leaf is expanded again, and a drag has no business editing the
    /// width a hidden panel is keeping for itself. That was the whole point of the 28.08 fix
    /// this file records, arrived at from the other side.
    ///
    /// Sizes come from the rectangles the layout actually gave the open children, rather than
    /// from weights and a row extent: `min_size` is in points, and the room a child has to give
    /// is what it *has* this frame — with fixed children in the row, the two are only the same
    /// number after subtracting exactly the strips and the dividers, which is the layout's
    /// arithmetic and not worth doing twice.
    ///
    /// Every mode is welcome here — the compacted vector is a row of open children, and `Chain`
    /// walking off the end of it walks off the end of the open ones, which is the honest reading:
    /// a strip has nothing to give.
    #[allow(clippy::too_many_arguments)]
    fn drag_across_strips(
        &mut self,
        gap: GapPath,
        near: usize,
        far: usize,
        pixels_per_point: f32,
        min_size: f32,
        delta: f32,
        behavior: &SepBehavior,
    ) -> bool {
        if delta == 0.0 {
            return false;
        }
        let Some(RowExtents { extents, .. }) = self.row_extents(gap.row) else {
            return false;
        };
        let horizontal = self.dock_state[gap.row].is_horizontal();
        let children = self.child_paths(gap.row);
        let open: Vec<usize> = (0..extents.len())
            .filter(|&index| matches!(extents[index], Extent::Weighted(_)))
            .collect();
        // The pair came from `trading_pair`, which found them in this same vector.
        let held = open
            .iter()
            .position(|&index| index == near)
            .expect("the near child of a trading pair takes a share");
        debug_assert_eq!(open.get(held + 1), Some(&far), "and the far one follows it");

        let row = self.dock_state[gap.row]
            .get_row()
            .expect("`trading_pair` answered, so this node is a row");
        let mut weights: Vec<f32> = open.iter().map(|&index| row.shares()[index].0).collect();
        // A child that was never laid out has no length to give, which `shrink_shares` reads as
        // "nothing to spare" — the same answer it gives a child already at its minimum.
        let sizes: Vec<f32> = open
            .iter()
            .map(|&index| {
                let rect = split_rect(
                    self.layout.rect(children[index]).unwrap_or(Rect::ZERO),
                    pixels_per_point,
                );
                if horizontal {
                    rect.width()
                } else {
                    rect.height()
                }
            })
            .collect();

        apply_drag(behavior, &mut weights, held, delta, min_size, |child| {
            sizes[child]
        });

        let mut shares = row.shares().to_vec();
        for (&index, weight) in open.iter().zip(weights) {
            shares[index] = Share(weight);
        }
        // A row with nothing left to give changes no weight, and reporting that as a write would
        // put a commit event behind a layout that did not move.
        if shares == row.shares() {
            return false;
        }
        self.mutations.push(DockMutation::SetShares {
            row: gap.row,
            shares,
        });
        true
    }

    /// Moves the boundary in `gap` by `delta` points along its row's own axis. Answers whether
    /// the stored ratio changed.
    ///
    /// **The only place a gesture writes a boundary** — see the note on [`SeparatorBand`]
    /// listing every writer there is and why that list is worth keeping short. `show_divider`
    /// calls it for the divider under the pointer, a junction handle for the two boundaries a
    /// corner is made of; both get the same clamp because it is the same function, not the same
    /// idea written twice.
    ///
    /// The delta is in **points**, not in fractions, and that is what makes a junction possible:
    /// the boundary is drawn at `near + range * effective`, so `delta / range` moves it by
    /// exactly `delta` points whatever interval it was cut from.
    fn nudge_boundary(
        &mut self,
        gap: GapPath,
        pixels_per_point: f32,
        extra: f32,
        delta: f32,
    ) -> bool {
        if delta == 0.0 {
            return false;
        }
        let Some((range, band)) = self.boundary_gesture(gap, pixels_per_point, extra) else {
            return false;
        };
        let at = (band.effective + delta / range).clamp(band.min, band.max);
        if self.boundary_at(gap) == at {
            return false;
        }
        self.mutations.push(DockMutation::SetBoundary { gap, at });
        true
    }
}

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

/// How wide a leaf collapsed sideways is: one tab bar's worth, turned on its side.
///
/// A tab bar's height and not a width of its own, so that a strip is exactly as thick as the
/// row a leaf collapses to under a vertical split — the same gesture, the same thickness,
/// whichever way the parent happens to be split. At the default style that is 24 px, which is
/// also [`Style::TAB_COLLAPSE_BUTTON_SIZE`], so the expand arrow fits a strip exactly.
///
/// `columns` for the same reason [`collapsed_strip_height`] takes `rows`, and with the same
/// arithmetic one axis over: a horizontal split collapsed leaf-by-leaf is a *row* of strips,
/// and the `columns - 1` dividers between them are part of what it draws. This used to be a
/// constant, on the stated grounds that "strips do not stack" — which was true only because
/// the rule that decided what became a strip could not see past a single pair, so a row of
/// three panels lost its strips entirely the moment the second one was collapsed.
pub(super) fn collapsed_strip_width(columns: i32, style: &Style) -> f32 {
    if columns <= 0 {
        return 0.0;
    }
    columns as f32 * style.tab_bar.height + (columns - 1) as f32 * style.separator.width
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

/// The rectangle a row's children are cut out of, snapped out to whole pixels.
///
/// A function, and called by both halves, because they have to name the *same* rectangle:
/// [`DockArea::compute_rect_sizes`] cuts the children out of it, and
/// [`DockArea::show_divider`] draws the divider between two of them against it, hit-tests it
/// there and moves it from there. They used to derive it separately — the layout pass snapped,
/// the separator did not — so the divider was measured against a slightly different node than
/// the children were: a boundary up to `2 / pixels_per_point` px off the gap it is supposed to
/// sit in, and a [`SeparatorBand`] computed from a different `range` on top of that. Sub-pixel
/// in every scene we could reach (measured: 0.08 px at `ppp = 1.3`, against 0.17 px of the
/// pixel-rounding both sides share anyway), which is exactly why it would have been found by
/// someone looking at the code rather than at the screen.
fn split_rect(node_rect: Rect, pixels_per_point: f32) -> Rect {
    debug_assert!(!node_rect.any_nan() && node_rect.is_finite());
    expand_to_pixel(node_rect, pixels_per_point)
}

/// Everything the layout pass decided about one row, in one value.
///
/// The type exists to make a branch unable to stay silent. Cutting a row answers three
/// questions at once — where each child goes, whether there is a divider in each gap and where,
/// and which children became sideways strips — and they are *one* decision: the branch that
/// gives a collapsed child exactly the strip it needs is the same branch that thereby leaves no
/// line at the ratio beside it. When each answer was written separately, wherever that branch
/// happened to end, adding a branch meant remembering all three, and the sideways one remembered
/// two. As fields, the compiler remembers for you.
///
/// Lists, one entry per child or per gap, and [`DockArea::compute_rect_sizes`] checks their
/// lengths against the row before writing a single one: a `zip` that truncated quietly would put
/// the branch that forgets a child back on the map.
///
/// Deliberately not `Option<Rect>` per child or a builder: there is no half-cut row, and
/// nothing here is allowed to be "left as it was".
#[derive(Clone, Debug)]
struct RowCut {
    /// Rectangles of the children, in [`DockArea::child_paths`] order.
    children: Vec<Rect>,

    /// Per gap, in order: the line between its two neighbours, or [`None`] if this cut left none
    /// there — see [`DockLayout::divider`].
    dividers: Vec<Option<Rect>>,

    /// Per child, in the same order as [`Self::children`]: which edge it was pressed against,
    /// for a child this cut squeezed into a sideways strip that draws its own bar.
    ///
    /// [`None`] for a child that is a row of strips rather than one, so that the mark lands on
    /// the leaves that draw the bars (see `DockArea::draws_one_bar`) — and for everything that
    /// is not a strip at all.
    side_strips: Vec<Option<SideStrip>>,
}

/// What a row whose children do not all take a share asks of each of them — see
/// [`DockArea::row_extents`], which is the only thing that builds one.
#[derive(Clone, Debug)]
struct RowExtents {
    /// One per child, in [`DockArea::child_paths`] order.
    extents: Vec<Extent>,

    /// Set for a **horizontal** row with strip columns in it, clear for a **vertical** row with
    /// collapsed children. Two things follow from it and nothing else does: which way the run is
    /// snapped (see [`cut_runs`], where the difference is inherited rather than chosen), and
    /// whether the children that became strips are marked as such — only a sideways strip draws
    /// something other than what it would draw anyway.
    sideways: bool,
}

/// How one child takes its length along the row's axis, as [`cut_runs`] sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Extent {
    /// Exactly this many points, whatever the row has to give: a collapsed child of a vertical
    /// row (its rows of tab bars — [`collapsed_strip_height`]), or a child of a horizontal row
    /// that fits in sideways strips ([`collapsed_strip_width`]).
    Fixed(f32),

    /// A share of what the fixed children leave, by this weight: an open child.
    Weighted(f32),
}

/// Where a child landed in a strip-aware cut — see [`cut_runs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Run {
    /// Among the fixed children pressed against the row's near edge (top / left), one after
    /// the other.
    Leading,

    /// Between the two runs: the open children sharing what is left, and any fixed child that
    /// sits among them.
    Middle,

    /// Among the fixed children pressed against the row's far edge (bottom / right).
    Trailing,
}

/// One child's place along the axis: the position it is cut at on each side, or `None` for the
/// row's own edge, left uncut.
type Span = (Option<f32>, Option<f32>);

/// What [`cut_runs`] decided, in one dimension.
struct RunCut {
    /// Per child, in order.
    spans: Vec<Span>,

    /// Per gap: the two edges of a divider to draw there, or `None` where the gap was cut at a
    /// fixed child's edge and there is no line at a ratio to draw or to grab.
    dividers: Vec<Option<(f32, f32)>>,

    /// Per child: which run it landed in.
    runs: Vec<Run>,
}

/// Cuts a row whose children do not all take a share: the fixed ones are given exactly their
/// length, and what is left goes to the open ones.
///
/// The one arithmetic behind two branches of [`DockArea::cut_row`]. A vertical row with a
/// collapsed child and a horizontal row with a sideways strip are the same problem one axis
/// over, and the pair-shaped code had solved it twice — which is how the 30.08 bug lived in one
/// axis and not the other (`a_row_collapses_panel_by_panel`).
///
/// **Runs.** Fixed children at the start of the row are stacked from its near edge (`lo`), one
/// after the other; fixed children at its end are stacked from its far edge (`hi`). Whatever
/// lies between — the open children, and any fixed child among them — shares the span the two
/// runs leave: a fixed child keeps its length, the open ones split the rest by weight (or
/// equally, if their weights add up to nothing: a proportion of nothing is not a proportion,
/// and an equal share is the least-bad answer, as [`SeparatorBand`] answers the centre when it
/// has no room to give). With no open child at all everything is one leading run, and
/// `last_fixed_takes_the_rest` says what happens to the row's far side: `false` cuts the last
/// child at its own length and the rest of the row is nobody's (the horizontal decision of
/// 30.08); `true` leaves it at the row's edge (the vertical rule the pair had).
///
/// **Snapping.** Every position handed back has been through `cut`, which puts it on the pixel
/// grid. `carry` is what a position goes through before the *next* one is computed from it, and
/// it is the one place the two axes differ — inherited, not chosen. The horizontal branch
/// snapped its run (`right_start = cut_at(left_end + separator)` with `left_end` already
/// snapped); the vertical one snapped each edge from the unsnapped run (`far = near + separator`
/// in points, then snapped). At an integer `pixels_per_point` the two agree; at a fractional
/// one they can land a pixel apart, so a parity stage hands each branch its own. Pinned by
/// `the_two_axes_snap_their_runs_differently_and_it_is_inherited`: unifying them is a decision
/// about pixels, not a cleanup.
///
/// **Dividers.** A divider is recorded wherever two open children have **only fixed children
/// between them**, and it is recorded at *both* edges of what lies between: one line where the
/// near open child ends, one where the far one begins. Two adjacent open children are the same
/// rule with nothing in between, and the two lines are then one — which is what every row
/// without a strip in it is, so nothing moves there.
///
/// Both lines mean the same trade: the two open children divide what the fixed ones leave, and
/// the strip between them rides along at its own width. A drag reads that pairing back out of
/// [`DockArea::trading_pair`], which walks the same extents.
///
/// This used to be "only between two open neighbours", and a strip in the *middle* of a row then
/// killed both of its gaps at once: the two open columns either side of it had no line between
/// them anywhere, and no way to be resized against each other at all. A strip at the row's *end*
/// still has no line beside it, and wants none — there is only one open child there, and nothing
/// for it to trade with.
///
/// # Panics
///
/// If the row holds fewer than two children — a bug in the caller, which built `extents` from a
/// row.
fn cut_runs(
    lo: f32,
    hi: f32,
    extents: &[Extent],
    separator: f32,
    cut: impl Fn(f32) -> f32,
    carry: impl Fn(f32) -> f32,
    last_fixed_takes_the_rest: bool,
) -> RunCut {
    let n = extents.len();
    assert!(n >= 2, "a row of {n} has nothing to cut between");
    let is_open = |index: usize| matches!(extents[index], Extent::Weighted(_));
    let fixed = |index: usize| match extents[index] {
        Extent::Fixed(length) => length,
        Extent::Weighted(_) => unreachable!("child {index} is in a run of fixed children"),
    };

    // The leading run is everything before the first open child, the trailing run everything
    // after the last one. With no open child there is no trailing run: the whole row is the
    // leading one.
    let first_open = (0..n).find(|&index| is_open(index));
    let leading_end = first_open.unwrap_or(n);
    let trailing_start = match first_open {
        Some(_) => {
            (0..n)
                .rev()
                .find(|&index| is_open(index))
                .expect("an open child was found going forward")
                + 1
        }
        None => n,
    };

    let mut spans: Vec<Span> = vec![(None, None); n];
    let mut dividers = vec![None; n - 1];
    let mut runs = vec![Run::Middle; n];

    // Down from the near edge. `cursor` is where the next child begins, carried the way the
    // caller asked; each child's edges are snapped from it.
    let mut cursor = lo;
    for index in 0..leading_end {
        runs[index] = Run::Leading;
        let end = cursor + fixed(index);
        let last = index == n - 1;
        spans[index] = (
            (index > 0).then(|| cut(cursor)),
            (!(last && last_fixed_takes_the_rest)).then(|| cut(end)),
        );
        cursor = carry(carry(end) + separator);
    }
    let top = cursor;

    // Up from the far edge, the same thing mirrored.
    let mut cursor = hi;
    for index in (trailing_start..n).rev() {
        runs[index] = Run::Trailing;
        let start = cursor - fixed(index);
        spans[index] = (Some(cut(start)), (index < n - 1).then(|| cut(cursor)));
        cursor = carry(carry(start) - separator);
    }
    let bottom = cursor;

    // Between the runs.
    let middle = leading_end..trailing_start;
    if !middle.is_empty() {
        let fixed_total: f32 = middle
            .clone()
            .filter(|&index| !is_open(index))
            .map(fixed)
            .sum();
        let (open_count, weight_total) = middle
            .clone()
            .filter_map(|index| match extents[index] {
                Extent::Weighted(weight) => Some(weight),
                Extent::Fixed(_) => None,
            })
            .fold((0usize, 0.0f32), |(count, sum), weight| {
                (count + 1, sum + weight)
            });
        let separators = (middle.len() - 1) as f32 * separator;
        let free = (bottom - top - fixed_total - separators).max(0.0);

        let mut cursor = top;
        for index in middle.clone() {
            let length = match extents[index] {
                Extent::Fixed(length) => length,
                Extent::Weighted(weight) if weight_total > 0.0 => free * weight / weight_total,
                Extent::Weighted(_) => free / open_count as f32,
            };
            let end = cursor + length;
            let last = index == n - 1;
            let last_of_the_middle = index + 1 == trailing_start;
            spans[index] = (
                (index > 0).then(|| cut(cursor)),
                if last {
                    None
                } else if last_of_the_middle {
                    // Where the trailing run begins: `bottom` itself, and not `cursor + length`,
                    // which is the same number only up to rounding. On a pair this child is
                    // the whole middle, and that difference is the whole of parity here.
                    Some(cut(bottom))
                } else {
                    Some(cut(end))
                },
            );
            // A line at this gap when it is an *outer* edge of what separates two open
            // children: the near child is open and somebody open is still ahead (the near
            // edge of the run of strips), or the far child is open and somebody open is
            // behind (its far edge). With nothing in between both clauses name the same gap
            // and it gets its one line, exactly as before.
            //
            // Guarded by `last_of_the_middle` first and not merely also: past the middle there
            // is no gap `index` to record and no child `index + 1` in the middle to ask about.
            let spans_a_trade = !last_of_the_middle && {
                let open_behind = middle.clone().any(|k| k <= index && is_open(k));
                let open_ahead = middle.clone().any(|k| k > index && is_open(k));
                (is_open(index) && open_ahead) || (is_open(index + 1) && open_behind)
            };
            if spans_a_trade {
                dividers[index] = Some((cut(end), cut(carry(carry(end) + separator))));
            }
            cursor = carry(carry(end) + separator);
        }
    }

    RunCut {
        spans,
        dividers,
        runs,
    }
}

/// The band a row's boundary may occupy this frame, and where the stored ratio sits inside it.
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
/// the band to the stored ratio on every frame — which is what this code used to do, drag or
/// no drag — turns a window resize into a silent edit of the layout: on a node shorter than
/// `2 * extra` the band is the single point `0.5`, so the ratio the user set is replaced by dead
/// centre and growing the window back does not bring it home. A ratio is state; only a gesture
/// gets to change it. Geometry gets to decide where it is honoured.
///
/// # Everything that writes a boundary, and whether it asks
///
/// The clamp is applied when the boundary is *drawn* and is never written back, so a ratio the
/// band cannot hold is not an error anywhere — it is simply drawn somewhere else than it says.
/// That makes every writer a place where the tree and the screen can quietly part company, and
/// there are only four of them:
///
/// * [`DockArea::nudge_boundary`] — every gesture that moves a boundary, whether the drag and
///   the arrow keys in [`DockArea::show_divider`] or a drag on a junction handle, which moves two
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
/// lists go stale — but [`TreeViolation::RowShareNegative`](crate::TreeViolation), which catches
/// the arithmetic that answers outside the row it was measuring, wherever it is written from: a
/// boundary written past either end of a row leaves a negative weight on the child that lost.

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
    /// The band for a boundary that has the whole row to itself: `range` is the node's extent
    /// along the split axis, `extra` is
    /// [`SeparatorStyle::extra`](crate::SeparatorStyle::extra); both in points.
    ///
    /// What every boundary had until a row could hold three, and what **drawing** still uses:
    /// each boundary is pushed into the row's own margins on its own, and the row's boundaries
    /// being monotone (running sums of non-negative weights) keeps their order. See
    /// [`between`](Self::between) for the gesture's band, which is narrower.
    fn new(fraction: f32, range: f32, extra: f32) -> Self {
        Self::between(fraction, 0.0, 1.0, range, extra)
    }

    /// The band for a boundary hemmed in by its **neighbours**: `lo` and `hi` are the boundaries
    /// on either side of it, `0.0` and `1.0` at the ends of the row.
    ///
    /// A gesture writes through this one, and on a row of three that is not a refinement but the
    /// difference between a valid tree and an invalid one. `RowNode::set_boundary` gives child
    /// `k` the room between `lo` and where the boundary lands and child `k + 1` the room up to
    /// `hi`; a boundary written past either neighbour therefore leaves one of them a **negative
    /// weight**, which is `TreeViolation::RowShareNegative`. On a pair the two neighbours are the
    /// row's own ends, so this could not arise and the whole row was the right band — which is
    /// why it was the only one for six stages.
    ///
    /// Found by the DST sweep at stage 7 rather than by reading: `DragSeparator` pulled gap 0 of
    /// a row of three past gap 1 and the oracle reported the violation.
    fn between(fraction: f32, lo: f32, hi: f32, range: f32, extra: f32) -> Self {
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
                min: lo,
                max: hi,
                effective: fraction,
            };
        }

        // Capping the margin at half the *room between the neighbours* is what makes an
        // impossible margin degrade sensibly: the band shrinks to a point and the boundary sits
        // at the centre of that room — an equal split of what there is, which is the least-bad
        // answer when there is none to give. The previous normalisation `(min.min(max),
        // max.max(min))` instead *swapped* the inverted pair, so `extra / range >= 1` produced
        // the interval `(0, 1)`: no constraint at all, exactly on the nodes where it was the
        // only thing standing between a child and zero size. Found by the frame harness — a drag
        // on a 175 px node drove `fraction` to 0.0.
        //
        // `room` is the whole row for a pair, so this is the same arithmetic it always was.
        //
        // The capped case is written as *one* point rather than as two ends that happen to meet,
        // and that is not tidiness: `lo + room/2` and `hi - room/2` are the same number in
        // arithmetic and need not be in `f32`. The sweep found them 3e-8 apart the wrong way
        // round (`min = 0.26666668`, `max = 0.26666665`) on a squeezed window, and
        // `f32::clamp` **panics** when its min exceeds its max — so the crate went down inside a
        // junction drag, on a row whose two neighbours had been squeezed to the margin.
        let room = (hi - lo).max(0.0);
        let margin = extra / range;
        let half = room * 0.5;
        let (min, max) = if margin >= half {
            let centre = lo + half;
            (centre, centre)
        } else {
            (lo + margin, hi - margin)
        };
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
    use egui::{Atoms, CentralPanel, Context, Id, Pos2, RawInput, Rect, Ui, Vec2};

    use super::{
        Extent, Run, SeparatorBand, collapsed_strip_height, collapsed_strip_width, cut_runs,
    };
    use crate::layout::{DockLayout, SideStrip};
    use crate::{
        DockArea, DockState, GapIndex, GapPath, Node, NodeId, NodePath, Share, Split, Style,
        SurfaceIndex, TabViewer, Tree,
    };

    /// Half a device pixel at the default scale: every boundary is snapped to whole pixels, so
    /// an exact comparison would be reporting the snapping rather than the property.
    const TOLERANCE: f32 = 0.5;

    const SCREEN: Vec2 = Vec2::new(1200.0, 900.0);
    const DOCK_ID: &str = "cut_row_test_dock";

    struct Viewer;

    impl TabViewer for Viewer {
        type Tab = u32;

        fn title(&mut self, tab: &Self::Tab) -> Atoms<'static> {
            Atoms::new(tab.to_string())
        }

        fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab) {
            ui.label(tab.to_string());
        }
    }

    fn style() -> Style {
        Style::from_egui(&egui::Style::default())
    }

    /// A dock whose main surface is one row of three leaves, built by hand — see
    /// `Tree::row_by_hand` for why by hand. Returns the dock, the row and its leaves in order.
    fn row_of_three(horizontal: bool, shares: [f32; 3]) -> (DockState<u32>, NodeId, [NodeId; 3]) {
        let (tree, row, leaves) = Tree::row_by_hand(
            horizontal,
            vec![vec![0u32], vec![1], vec![2]],
            shares.into_iter().map(Share).collect(),
        );
        let mut state = DockState::new(vec![9u32]);
        *state.main_surface_mut() = tree;
        assert_eq!(
            state.validate(),
            Ok(()),
            "the hand-built row is a legal dock"
        );
        (state, row, [leaves[0], leaves[1], leaves[2]])
    }

    /// A few headless frames, and the geometry they settled on.
    fn lay_out(state: &mut DockState<u32>, style: &Style, sideways: bool) -> DockLayout {
        let ctx = Context::default();
        let id = Id::new(DOCK_ID);
        for _ in 0..4 {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                CentralPanel::default().show(ui, |ui| {
                    DockArea::new(state)
                        .id(id)
                        .style(style.clone())
                        .show_leaf_collapse_buttons(true)
                        .collapse_sideways(sideways)
                        .show_inside(ui, &mut Viewer);
                });
            });
            output.textures_delta.clear();
        }
        DockLayout::load(&ctx, id)
    }

    fn main(node: NodeId) -> NodePath {
        NodePath::new(SurfaceIndex::main(), node)
    }

    fn rect_of(layout: &DockLayout, node: NodeId) -> Rect {
        layout.rect(main(node)).expect("the node was laid out")
    }

    fn divider_of(layout: &DockLayout, row: NodeId, gap: usize) -> Option<Rect> {
        layout.divider(GapPath::new(main(row), GapIndex(gap)))
    }

    fn close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() <= TOLERANCE,
            "{what}: expected {expected}, got {actual}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // `cut_runs`, in one dimension. Stated on the arithmetic, where the property lives: a pair
    // never has a middle of two, a fixed child among open ones, or a trailing run beside a
    // leading one, so nothing on screen can reach these until stage 7 — and the corpus probes,
    // which judge parity on pairs, cannot tell a right n-ary cut from a wrong one.
    // ---------------------------------------------------------------------------------------

    fn whole(at: f32) -> f32 {
        at.round()
    }

    /// Fixed children at both ends stack from their own edges; the open ones between them share
    /// what the runs leave, and the one divider is between the two open neighbours.
    #[test]
    fn a_leading_run_a_middle_and_a_trailing_run() {
        let extents = [
            Extent::Fixed(10.0),
            Extent::Weighted(1.0),
            Extent::Weighted(1.0),
            Extent::Fixed(10.0),
        ];
        let cut = cut_runs(0.0, 100.0, &extents, 2.0, whole, whole, false);
        assert_eq!(
            cut.spans,
            vec![
                (None, Some(10.0)),
                (Some(12.0), Some(49.0)),
                (Some(51.0), Some(88.0)),
                (Some(90.0), None),
            ]
        );
        assert_eq!(cut.dividers, vec![None, Some((49.0, 51.0)), None]);
        assert_eq!(
            cut.runs,
            vec![Run::Leading, Run::Middle, Run::Middle, Run::Trailing]
        );
    }

    /// With nothing open the whole row is a leading run, and whether the last child reaches the
    /// far edge is the caller's decision — the vertical rule says yes, the horizontal one (Стас,
    /// 30.08: strips for everyone, the rest empty) says no.
    #[test]
    fn with_nothing_open_the_last_fixed_child_keeps_the_rest_only_if_asked() {
        let extents = [Extent::Fixed(10.0); 3];
        let keeps = cut_runs(0.0, 100.0, &extents, 2.0, whole, whole, true);
        assert_eq!(
            keeps.spans,
            vec![
                (None, Some(10.0)),
                (Some(12.0), Some(22.0)),
                (Some(24.0), None)
            ]
        );
        let leaves = cut_runs(0.0, 100.0, &extents, 2.0, whole, whole, false);
        assert_eq!(
            leaves.spans,
            vec![
                (None, Some(10.0)),
                (Some(12.0), Some(22.0)),
                (Some(24.0), Some(34.0)),
            ]
        );
        for cut in [&keeps, &leaves] {
            assert_eq!(cut.runs, vec![Run::Leading; 3]);
            assert_eq!(cut.dividers, vec![None, None]);
        }
    }

    /// A fixed child between two open ones keeps exactly its length, the open ones split the
    /// rest, and **both** gaps beside it draw a line: the two open children have only a strip
    /// between them, so each of the strip's edges is a handle on the one boundary they share.
    ///
    /// The lines are at the strip's edges and not at a ratio — which is why the file used to say
    /// there were none at all. That left the two open children with no line between them
    /// anywhere and no way to be resized against each other, which is the defect this pins.
    #[test]
    fn a_fixed_child_among_open_ones_is_grabbed_by_both_its_edges() {
        let extents = [
            Extent::Weighted(1.0),
            Extent::Fixed(10.0),
            Extent::Weighted(1.0),
        ];
        let cut = cut_runs(0.0, 100.0, &extents, 2.0, whole, whole, false);
        assert_eq!(
            cut.spans,
            vec![
                (None, Some(43.0)),
                (Some(45.0), Some(55.0)),
                (Some(57.0), None)
            ]
        );
        assert_eq!(
            cut.dividers,
            vec![Some((43.0, 45.0)), Some((55.0, 57.0))],
            "one line where the near column ends, one where the far one begins"
        );
        assert_eq!(cut.runs, vec![Run::Middle; 3]);
    }

    /// A strip at the row's **end** still draws no line beside it — there is only one open child
    /// there, and nothing for it to trade with. The positive control for the test above: without
    /// it, "a strip has handles" would pass just as well if every gap beside a strip grew one.
    #[test]
    fn a_fixed_child_at_the_end_of_the_row_has_nothing_to_trade_with() {
        let leading = [
            Extent::Fixed(10.0),
            Extent::Weighted(1.0),
            Extent::Weighted(1.0),
        ];
        let cut = cut_runs(0.0, 100.0, &leading, 2.0, whole, whole, false);
        assert_eq!(
            cut.dividers[0], None,
            "no line between the strip and the column beside it"
        );
        assert!(
            cut.dividers[1].is_some(),
            "control: the two open columns still have theirs"
        );

        let trailing = [
            Extent::Weighted(1.0),
            Extent::Weighted(1.0),
            Extent::Fixed(10.0),
        ];
        let cut = cut_runs(0.0, 100.0, &trailing, 2.0, whole, whole, false);
        assert!(cut.dividers[0].is_some(), "control: the same, mirrored");
        assert_eq!(cut.dividers[1], None);
    }

    /// Open children whose weights add up to nothing are not a proportion of anything; they
    /// share equally rather than divide by zero.
    #[test]
    fn open_children_with_no_weight_between_them_share_equally() {
        let extents = [
            Extent::Fixed(10.0),
            Extent::Weighted(0.0),
            Extent::Weighted(0.0),
        ];
        let cut = cut_runs(0.0, 100.0, &extents, 2.0, whole, whole, false);
        assert_eq!(
            cut.spans,
            vec![
                (None, Some(10.0)),
                (Some(12.0), Some(55.0)),
                (Some(57.0), None)
            ]
        );
        assert_eq!(cut.dividers, vec![None, Some((55.0, 57.0))]);
    }

    /// **The two axes snap their runs differently, and it is inherited, not chosen.** The
    /// horizontal branch snaps the run itself (`right_start = cut_at(left_end + separator)`
    /// with `left_end` already snapped); the vertical one snaps each edge from the unsnapped
    /// run. At `pixels_per_point = 1` the two agree, which is why the corpus probes and the
    /// sweep cannot tell them apart; at a fractional scale they land a pixel apart. A parity
    /// stage keeps each branch its own scheme, and this pins that it did: change one to the
    /// other and a strip's divider moves a pixel on every HiDPI screen. Unifying them is a
    /// decision about pixels, not a cleanup — see `cut_runs`.
    #[test]
    fn the_two_axes_snap_their_runs_differently_and_it_is_inherited() {
        const PIXELS_PER_POINT: f32 = 1.5;
        let snap = |at: f32| (at * PIXELS_PER_POINT).round() / PIXELS_PER_POINT;
        let pixels = |at: f32| (at * PIXELS_PER_POINT).round();
        // Two fixed children, not one: the *second* strip's far edge is what tells "the run is
        // snapped" from "only the cut is" — with one strip the two are the same number.
        let extents = [
            Extent::Fixed(24.4),
            Extent::Fixed(24.4),
            Extent::Weighted(1.0),
        ];

        let vertical = cut_runs(0.0, 100.0, &extents, 1.2, snap, |at| at, true);
        let horizontal = cut_runs(0.0, 100.0, &extents, 1.2, snap, snap, false);

        // The first edge is the same number either way: it is snapped from the row's edge.
        assert_eq!(pixels(vertical.spans[0].1.unwrap()), 37.0);
        assert_eq!(pixels(horizontal.spans[0].1.unwrap()), 37.0);
        // The next one is not: 24.4 + 1.2 = 25.6 → pixel 38 from the unsnapped run, but
        // 24.667 (pixel 37) + 1.2 = 25.867 → pixel 39 from the snapped one.
        assert_eq!(
            pixels(vertical.spans[1].0.unwrap()),
            38.0,
            "the vertical branch snaps each edge from the run in points"
        );
        assert_eq!(
            pixels(horizontal.spans[1].0.unwrap()),
            39.0,
            "the horizontal branch snaps the run itself at every step"
        );
        // And the second strip's far edge carries the difference on: 25.6 + 24.4 = 50 → pixel
        // 75 in points, against 26 (pixel 39) + 24.4 = 50.4 → pixel 76 along the snapped run.
        assert_eq!(pixels(vertical.spans[1].1.unwrap()), 75.0);
        assert_eq!(
            pixels(horizontal.spans[1].1.unwrap()),
            76.0,
            "the horizontal run is carried snapped, so the next strip starts from a pixel"
        );
    }

    /// Open children between the runs split what the fixed ones leave **by weight**, not
    /// equally: with weights 1 and 3 the second takes three times the first.
    #[test]
    fn open_children_split_what_the_fixed_ones_leave_by_weight() {
        let extents = [
            Extent::Fixed(10.0),
            Extent::Weighted(1.0),
            Extent::Weighted(3.0),
        ];
        // 102 − 12 − 2 = 88 to share: 22 and 66.
        let cut = cut_runs(0.0, 102.0, &extents, 2.0, whole, whole, false);
        assert_eq!(
            cut.spans,
            vec![
                (None, Some(10.0)),
                (Some(12.0), Some(34.0)),
                (Some(36.0), None)
            ]
        );
        assert_eq!(cut.dividers, vec![None, Some((34.0, 36.0))]);
    }

    // ---------------------------------------------------------------------------------------
    // The same three branches on screen, on rows of three built by hand.
    // ---------------------------------------------------------------------------------------

    /// An open row of three is cut at both of its boundaries, where the weights put them, with
    /// a divider in each gap — and the three children plus the two dividers tile the row.
    #[test]
    fn a_row_of_three_is_cut_at_each_of_its_boundaries() {
        for horizontal in [true, false] {
            let style = style();
            let (mut state, row, leaves) = row_of_three(horizontal, [1.0, 1.0, 2.0]);
            let layout = lay_out(&mut state, &style, false);

            let whole = rect_of(&layout, row);
            let (lo, size) = if horizontal {
                (whole.min.x, whole.width())
            } else {
                (whole.min.y, whole.height())
            };
            let along = |rect: Rect| {
                if horizontal {
                    (rect.min.x, rect.max.x)
                } else {
                    (rect.min.y, rect.max.y)
                }
            };

            let first = divider_of(&layout, row, 0).expect("a divider in the first gap");
            let second = divider_of(&layout, row, 1).expect("a divider in the second gap");
            let centre = |rect: Rect| {
                let (near, far) = along(rect);
                (near + far) * 0.5
            };
            close(
                centre(first),
                lo + size * 0.25,
                "the first boundary, at 1/(1+1+2)",
            );
            close(
                centre(second),
                lo + size * 0.5,
                "the second boundary, at 2/(1+1+2)",
            );

            let [a, b, c] = leaves.map(|leaf| along(rect_of(&layout, leaf)));
            assert_eq!(
                a.0,
                along(whole).0,
                "the first child starts where the row starts"
            );
            assert_eq!(a.1, along(first).0, "the first child ends at its divider");
            assert_eq!(b.0, along(first).1);
            assert_eq!(b.1, along(second).0);
            assert_eq!(c.0, along(second).1);
            assert_eq!(
                c.1,
                along(whole).1,
                "the last child ends where the row ends"
            );
        }
    }

    /// A strip at each end of a horizontal row hugs its own edge — the first the left, the last
    /// the right — and the open column between them takes everything they gave up; no gap
    /// beside a strip draws a divider.
    #[test]
    fn strips_at_both_ends_of_a_row_hug_their_own_edges() {
        let style = style();
        let (mut state, row, [a, b, c]) = row_of_three(true, [1.0, 1.0, 1.0]);
        state.main_surface_mut().set_leaf_collapsed(a, true);
        state.main_surface_mut().set_leaf_collapsed(c, true);
        let layout = lay_out(&mut state, &style, true);

        let whole = rect_of(&layout, row);
        let strip = collapsed_strip_width(1, &style);
        let separator = style.separator.width;

        let left = rect_of(&layout, a);
        close(
            left.min.x,
            whole.min.x,
            "the first strip starts at the row's edge",
        );
        close(left.width(), strip, "and is one strip wide");
        assert_eq!(layout.side_strip(main(a)), Some(SideStrip::Left));

        let right = rect_of(&layout, c);
        close(
            right.max.x,
            whole.max.x,
            "the last strip ends at the row's edge",
        );
        close(right.width(), strip, "and is one strip wide");
        assert_eq!(layout.side_strip(main(c)), Some(SideStrip::Right));

        let open = rect_of(&layout, b);
        close(
            open.min.x,
            left.max.x + separator,
            "the column starts a separator after the strip",
        );
        close(
            open.max.x,
            right.min.x - separator,
            "and ends a separator before the other",
        );
        assert_eq!(layout.side_strip(main(b)), None);

        assert_eq!(
            divider_of(&layout, row, 0),
            None,
            "cut at the strip's edge, not at a ratio"
        );
        assert_eq!(divider_of(&layout, row, 1), None);
    }

    /// A strip between two open columns is exactly one strip wide where it stands, and the two
    /// columns share what it gave up **by weight** — three to one here, so that a cut that
    /// shared it equally is told apart from one that read the weights.
    #[test]
    fn a_strip_among_open_columns_hands_its_width_to_both_sides() {
        let style = style();
        let (mut state, row, [a, b, c]) = row_of_three(true, [1.0, 1.0, 3.0]);
        state.main_surface_mut().set_leaf_collapsed(b, true);
        let layout = lay_out(&mut state, &style, true);

        let strip = rect_of(&layout, b);
        close(
            strip.width(),
            collapsed_strip_width(1, &style),
            "one strip wide",
        );
        assert_eq!(layout.side_strip(main(b)), Some(SideStrip::Left));

        let left = rect_of(&layout, a);
        let right = rect_of(&layout, c);
        // Each edge is snapped on its own, so the ratio holds to a couple of pixels.
        assert!(
            (right.width() - 3.0 * left.width()).abs() <= 2.0,
            "weights 1 and 3: the right column is three times the left, got {} and {}",
            left.width(),
            right.width()
        );
        close(
            left.max.x + style.separator.width,
            strip.min.x,
            "the strip starts after the left column",
        );
        close(
            strip.max.x + style.separator.width,
            right.min.x,
            "and the right one after the strip",
        );
        // The two columns share one boundary and it has a handle at each edge of the strip
        // between them — a strip in the middle used to leave them with none anywhere.
        close(
            divider_of(&layout, row, 0).unwrap().max.x,
            strip.min.x,
            "the near handle sits where the strip begins",
        );
        close(
            divider_of(&layout, row, 1).unwrap().min.x,
            strip.max.x,
            "and the far one where it ends",
        );
    }

    /// Every column a strip: they sit against the left edge one after the other, each marked
    /// `Left`, and the rest of the row is nobody's — the last strip does *not* stretch to the far
    /// edge (Стас, 30.08).
    #[test]
    fn with_every_column_a_strip_the_rest_of_the_row_is_nobodys() {
        let style = style();
        let (mut state, row, leaves) = row_of_three(true, [1.0, 1.0, 1.0]);
        for leaf in leaves {
            state.main_surface_mut().set_leaf_collapsed(leaf, true);
        }
        let layout = lay_out(&mut state, &style, true);

        let whole = rect_of(&layout, row);
        let strip = collapsed_strip_width(1, &style);
        let step = strip + style.separator.width;
        for (index, leaf) in leaves.into_iter().enumerate() {
            let rect = rect_of(&layout, leaf);
            close(
                rect.min.x,
                whole.min.x + step * index as f32,
                "strips one after the other",
            );
            close(rect.width(), strip, "each one strip wide");
            assert_eq!(layout.side_strip(main(leaf)), Some(SideStrip::Left));
        }
        let last = rect_of(&layout, leaves[2]);
        assert!(
            whole.max.x - last.max.x > 100.0,
            "the rest of the row belongs to nobody: the last strip ends at {} of {}",
            last.max.x,
            whole.max.x
        );
        assert_eq!(divider_of(&layout, row, 0), None);
        assert_eq!(divider_of(&layout, row, 1), None);
    }

    /// Collapsed rows at both ends of a stack hang from their own edges — the first from the top,
    /// the last from the bottom — and the open one between them takes the rest.
    #[test]
    fn collapsed_rows_at_both_ends_of_a_stack_leave_the_open_one_between() {
        let style = style();
        let (mut state, row, [a, b, c]) = row_of_three(false, [1.0, 1.0, 1.0]);
        state.main_surface_mut().set_leaf_collapsed(a, true);
        state.main_surface_mut().set_leaf_collapsed(c, true);
        let layout = lay_out(&mut state, &style, false);

        let whole = rect_of(&layout, row);
        let bar = collapsed_strip_height(1, &style);
        let separator = style.separator.width;

        let top = rect_of(&layout, a);
        close(
            top.min.y,
            whole.min.y,
            "the first collapsed row starts at the top",
        );
        close(top.height(), bar, "and is one tab bar tall");
        let bottom = rect_of(&layout, c);
        close(
            bottom.max.y,
            whole.max.y,
            "the last collapsed row ends at the bottom",
        );
        close(bottom.height(), bar, "and is one tab bar tall");
        let open = rect_of(&layout, b);
        close(
            open.min.y,
            top.max.y + separator,
            "the open one starts a separator below the first",
        );
        close(
            open.max.y,
            bottom.min.y - separator,
            "and ends a separator above the last",
        );
        assert_eq!(
            divider_of(&layout, row, 0),
            None,
            "cut at the bar's edge, not at a ratio"
        );
        assert_eq!(divider_of(&layout, row, 1), None);
    }

    /// A vertical row whose *first* child is itself a stack of collapsed rows is given a bar for
    /// **each** of them — `collapsed_leaf_count`, not one — with the separator between.
    ///
    /// Found missing by mutation at stage 6, and it predates the stage: the arithmetic was
    /// there, and no scene in the suite or in the corpus (three scenes × 544 layouts) had a
    /// fully collapsed vertical row as the first child of a vertical row. As the *last* child it
    /// takes the rest of the column and its height is never asked, which is where every such
    /// scene had it. A mutant handing every collapsed child one bar passed everything.
    #[test]
    fn a_stack_of_collapsed_rows_is_given_a_bar_for_each_of_them() {
        let style = style();
        let mut state = DockState::new(vec![0u32]);
        let a = state.main_surface().root().unwrap();
        // V(V(b, c), a). Built by hand at the inner step, because no gesture makes this shape
        // any more: a second `Split::Above` would join the row it is in and give `V(c, b, a)`,
        // which is the point of stage 7. The shape is still reachable — loading keeps a stowed
        // row, a regrouping can leave one — so its layout is still worth pinning.
        let [_, stack] = state.split(main(a), Split::Above, 0.5, Node::leaf(1u32));
        let (inner, leaves) =
            state
                .main_surface_mut()
                .nest_row_by_hand(stack, false, vec![vec![1u32], vec![2u32]]);
        let (b, c) = (leaves[0], leaves[1]);
        let outer = state.main_surface().root().unwrap();
        assert_ne!(inner, outer, "the stack is nested, not the root");
        state.main_surface_mut().set_leaf_collapsed(b, true);
        state.main_surface_mut().set_leaf_collapsed(c, true);
        let layout = lay_out(&mut state, &style, false);

        let bar = collapsed_strip_height(1, &style);
        let stack = rect_of(&layout, inner);
        close(
            stack.height(),
            collapsed_strip_height(2, &style),
            "two bars and the separator between them",
        );
        close(rect_of(&layout, b).height(), bar, "the first bar");
        close(
            rect_of(&layout, c).height(),
            bar,
            "the second bar — not the rest of the column, and not nothing",
        );
        close(
            rect_of(&layout, a).min.y,
            stack.max.y + style.separator.width,
            "the open panel starts a separator below the stack",
        );
        assert_eq!(
            divider_of(&layout, outer, 0),
            None,
            "cut at the stack's edge"
        );
    }

    /// **A child squeezed to nothing is given nothing, never less than nothing.**
    ///
    /// Two boundaries of a row can land on the same point — a child of weight zero, or two
    /// boundaries the margin pushes into the same limit — and the child between them is then
    /// asked to fit between the far edge of one divider and the near edge of the next, which is
    /// a divider's width *backwards*. An inverted rectangle is not a small rectangle: it is a
    /// panel whose hit test and clipping disagree about which side of itself it is on.
    ///
    /// Unreachable while a row held two — one boundary has nothing to coincide with — so stage 6
    /// wrote the branch and recorded the hole. Stage 7 made it reachable and the corpus probe
    /// found it: 86 inside-out rectangles across 544 layouts, all in chains flattened on load.
    /// Stated here as well, because a defect found by a probe run by hand is a defect nothing
    /// runs again.
    ///
    /// A minimum size per child would be the other answer, and is deliberately not this feature
    /// — see "What this does not do" in the plan.
    ///
    /// # Why four scenes and not one
    ///
    /// The repair is two passes, one from each end, and with the default margin **either one
    /// alone** keeps this row honest — so a single scene would pin the pair while judging
    /// neither. Zeroing `separator.extra` takes the margin's help away and separates them: the
    /// weights piled at the far end walk the last divider past the row's edge, which only the
    /// backward pass pulls in, and the weights piled at the near end walk the first one before
    /// it, which only the forward pass does.
    #[test]
    fn a_child_squeezed_between_two_boundaries_is_given_nothing_not_less() {
        let mut no_margin = style();
        no_margin.separator.extra = 0.0;

        // Every child asking for nothing puts the boundaries on the near edge; every child but
        // the first asking for nothing puts them on the far edge.
        for (label, style) in [("with a margin", style()), ("without one", no_margin)] {
            for shares in [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0]] {
                for horizontal in [true, false] {
                    let where_at = format!("{label}, {shares:?}, horizontal={horizontal}");
                    let (mut state, row, leaves) = row_of_three(horizontal, shares);
                    let layout = lay_out(&mut state, &style, false);

                    let whole = rect_of(&layout, row);
                    for leaf in leaves {
                        let rect = rect_of(&layout, leaf);
                        assert!(
                            rect.max.x >= rect.min.x && rect.max.y >= rect.min.y,
                            "{where_at}: a leaf came out inside out: {rect:?}"
                        );
                        assert!(
                            whole.contains_rect(rect),
                            "{where_at}: {rect:?} is not inside its row {whole:?}"
                        );
                    }
                }
            }
        }
    }

    /// Every row collapsed: the stack hangs from the top, one tab bar each with a separator
    /// between, and **the rest of the column is nobody's** — the same answer as the horizontal
    /// row above.
    ///
    /// It used to be the other way on this axis: the last child kept the rest, which the pair
    /// had done and which stage 6 kept by parity while recording that the two axes disagreed.
    /// Same picture either way — a collapsed leaf draws its bar at the top of whatever it is
    /// given — but not the same answer to a hit test or a drop target, and a strip that is
    /// silently a whole column is not a strip. Reconciled at stage 7 (decision 7, Стас); this
    /// test is the one the reconciliation rewrote, and it named the old answer on purpose so
    /// that it would have to be.
    #[test]
    fn with_every_row_collapsed_the_stack_hangs_from_the_top_and_the_rest_is_nobodys() {
        let style = style();
        let (mut state, row, leaves) = row_of_three(false, [1.0, 1.0, 1.0]);
        for leaf in leaves {
            state.main_surface_mut().set_leaf_collapsed(leaf, true);
        }
        let layout = lay_out(&mut state, &style, false);

        let whole = rect_of(&layout, row);
        let bar = collapsed_strip_height(1, &style);
        let step = bar + style.separator.width;
        for (index, leaf) in leaves.into_iter().enumerate() {
            let rect = rect_of(&layout, leaf);
            close(
                rect.min.y,
                whole.min.y + step * index as f32,
                "bars one after the other",
            );
        }
        close(
            rect_of(&layout, leaves[0]).height(),
            bar,
            "the first is one bar tall",
        );
        close(
            rect_of(&layout, leaves[1]).height(),
            bar,
            "so is the second",
        );
        close(
            rect_of(&layout, leaves[2]).height(),
            bar,
            "and so is the last — not the rest of the column",
        );
        assert!(
            whole.max.y - rect_of(&layout, leaves[2]).max.y > 100.0,
            "the rest of the column belongs to nobody: the last bar ends at {} of {}",
            rect_of(&layout, leaves[2]).max.y,
            whole.max.y
        );
        assert_eq!(divider_of(&layout, row, 0), None);
        assert_eq!(divider_of(&layout, row, 1), None);
    }

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

    /// **A band squeezed between two neighbours collapses to a point, and never past it.**
    ///
    /// `lo + room/2` and `hi - room/2` are one number in arithmetic and two in `f32`, and the
    /// order they come out in is not fixed: the sweep found them 3e-8 apart the wrong way round
    /// on a squeezed window, which made `f32::clamp` panic (`min > max`) inside a junction drag —
    /// the crate going down, not a boundary going astray. Fixed by writing the capped case as one
    /// point instead of two ends that ought to meet, and pinned here over a spread of positions
    /// and rooms rather than at the one triple that happened to fail.
    #[test]
    fn a_band_with_no_room_between_its_neighbours_is_a_single_point() {
        for lo in [0.0, 0.1, 0.26666668, 1.0 / 3.0, 0.7] {
            for room in [0.0, 1e-6, 0.05, 0.2] {
                let hi = lo + room;
                // A margin far larger than half the room, which is the regime the cap is for.
                let band = SeparatorBand::between(0.5 * (lo + hi), lo, hi, 100.0, 175.0);
                assert!(
                    band.min <= band.max,
                    "lo {lo}, room {room}: band came out inverted ({}, {})",
                    band.min,
                    band.max
                );
                assert_eq!(band.min, band.max, "lo {lo}, room {room}: not a point");
                // The clamp the callers run, which is what panicked.
                let _ = 0.42_f32.clamp(band.min, band.max);
            }
        }
    }
}
