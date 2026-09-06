//! The egui `Id`s the dock draws its interactive parts under.
//!
//! # Why this is public
//!
//! Same reason [`DockLayout`](crate::DockLayout) is: code that drives the dock from outside a
//! frame — automation, screenshots, diagnostics, our own deterministic simulator — has to be
//! able to *address* what the dock drew. Geometry alone is not enough. Aiming at a guessed
//! offset inside the tab bar was tried first and silently missed: the bar has leading buttons,
//! so a press at "16 px from the left edge" landed 8 px to the left of the first tab, moved
//! nothing, and every test stayed green.
//!
//! The scheme used to be re-derived at the call site instead, which is the shape this crate
//! has been bitten by more than once: the copy was *checked* (the harness fails loudly if the
//! id does not resolve), but a check is not a single source, and the check only exists in the
//! one harness that thought to write it.
//!
//! The id is an address, not a promise about layout: it stays stable as long as the tab and the
//! node holding it do.
//!
//! # Why the tab coordinate is an identity and not a position
//!
//! egui hangs per-widget state off the id — what is focused, hovered, dragged. An id built from
//! a tab's *position* in the bar is therefore reused the moment the bar is edited: close one tab
//! and its neighbour slides into the slot, inherits the id, and inherits everything egui had
//! attached to it. That has been a real bug once already (a drag surviving the close of the tab
//! it carried and continuing on the neighbour — see FINDINGS), and the same reuse hands over
//! focus and hover just as quietly.
//!
//! [`TabId`] is handed out per leaf and never reused, so the address follows the tab through
//! edits of its leaf, and an id whose tab is gone is an id nobody answers to — which is what
//! stale egui state should be.

use egui::{Context, Id};

use super::state::{DragInFlight, State};
use crate::{DockState, NodePath, TabId, TabPath};

/// Id of the widget one tab is drawn as, in the dock area with id `dock_area_id`.
///
/// `dock_area_id` is the id of the [`DockArea`](crate::DockArea) — the default is
/// `Id::new("egui_dockyard::DockArea")`, or whatever was passed to
/// [`DockArea::id`](crate::DockArea::id).
///
/// `tab` is the tab's identity inside its leaf, which
/// [`LeafNode::tab_id_at`](crate::LeafNode::tab_id_at) resolves from a position.
///
/// ```rust
/// # use egui_dockyard::{DockState, NodePath, SurfaceIndex, TabIndex, tab_widget_id};
/// # egui::__run_test_ctx(|ctx| {
/// let dock_state = DockState::new(vec!["a tab"]);
/// let dock_id = egui::Id::new("egui_dockyard::DockArea");
/// let leaf = dock_state.main_surface().root().unwrap();
/// let path = NodePath::new(SurfaceIndex::main(), leaf);
/// let tab = dock_state.leaf(path).unwrap().tab_id_at(TabIndex(0)).unwrap();
/// // What the dock drew that tab as — `ctx.read_response` answers where it ended up.
/// let _id = tab_widget_id(dock_id, path, tab);
/// # });
/// ```
pub fn tab_widget_id(dock_area_id: Id, path: NodePath, tab: TabId) -> Id {
    dock_area_id
        .with((path.surface, "surface"))
        .with((path.node, "node"))
        .with((tab, "tab"))
}

/// The tab a drag is currently carrying, or `None` if no drag is in flight.
///
/// Reads the same per-frame state the dock keeps in `Context` memory; `dock_area_id` is the
/// `DockArea`'s id.
///
/// The dock addresses a drag's source by identity ([`TabId`]), not by position, and this
/// resolves that identity against `dock_state` as it stands *now* — so a drag whose tab left
/// the tree (closed by the user, force-closed by the application) answers `None`, exactly as
/// "no drag" does. That merge is deliberate: a caller cannot tell a stale drag from no drag,
/// which is also the one distinction the dock itself is not allowed to act differently on.
///
/// ```rust
/// # use egui::{
/// #     Atoms, CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
/// # };
/// # use egui_dockyard::{
/// #     DockArea, DockState, NodePath, Style, SurfaceIndex, TabIndex, TabPath, TabViewer,
/// #     dragged_tab, tab_widget_id,
/// # };
/// #
/// # struct Viewer;
/// # impl TabViewer for Viewer {
/// #     type Tab = String;
/// #     fn title(&mut self, tab: &String) -> Atoms<'static> {
/// #         Atoms::new(tab.clone())
/// #     }
/// #     fn ui(&mut self, ui: &mut Ui, tab: &String) {
/// #         ui.label(tab.as_str());
/// #     }
/// # }
/// #
/// # fn run(
/// #     ctx: &Context,
/// #     dock_id: Id,
/// #     state: &mut DockState<String>,
/// #     events: Vec<Event>,
/// #     frame: &mut u32,
/// # ) {
/// #     let input = RawInput {
/// #         screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
/// #         time: Some(f64::from(*frame) / 60.0),
/// #         events,
/// #         ..Default::default()
/// #     };
/// #     *frame += 1;
/// #     let mut output = ctx.run_ui(input, |ctx| {
/// #         CentralPanel::default().show(ctx, |ui| {
/// #             DockArea::new(state)
/// #                 .id(dock_id)
/// #                 .style(Style::from_egui(ui.style().as_ref()))
/// #                 .show_close_buttons(true)
/// #                 .show_inside(ui, &mut Viewer)
/// #                 .apply(ui.ctx(), state, &mut Viewer);
/// #         });
/// #     });
/// #     output.textures_delta.clear();
/// # }
/// #
/// # let dock_id = Id::new("doctest dock");
/// # let mut dock_state = DockState::new(vec!["a".to_owned(), "b".to_owned()]);
/// # let ctx = Context::default();
/// # let mut frame = 0u32;
/// # run(&ctx, dock_id, &mut dock_state, vec![], &mut frame);
/// # let leaf = NodePath::new(SurfaceIndex::main(), dock_state.main_surface().root().unwrap());
/// # let tab = dock_state.leaf(leaf).unwrap().tab_id_at(TabIndex(0)).unwrap();
/// # let rect = ctx.read_response(tab_widget_id(dock_id, leaf, tab)).unwrap().rect;
/// # // The centre of a short label sits on its close button, which answers a click before
/// # // the title does — aim at the title instead, same as `dst.rs` does.
/// # let home = Pos2::new(rect.left() + 4.0, rect.center().y);
/// # run(&ctx, dock_id, &mut dock_state, vec![Event::PointerMoved(home)], &mut frame);
/// # run(
/// #     &ctx,
/// #     dock_id,
/// #     &mut dock_state,
/// #     vec![Event::PointerButton {
/// #         pos: home,
/// #         button: PointerButton::Primary,
/// #         pressed: true,
/// #         modifiers: Default::default(),
/// #     }],
/// #     &mut frame,
/// # );
/// # let out = home + Vec2::new(0.0, 200.0);
/// # for step in 1..=8u8 {
/// #     let p = home + (out - home) * (f32::from(step) / 8.0);
/// #     run(&ctx, dock_id, &mut dock_state, vec![Event::PointerMoved(p)], &mut frame);
/// # }
/// # for step in (0..8u8).rev() {
/// #     let p = home + (out - home) * (f32::from(step) / 8.0);
/// #     run(&ctx, dock_id, &mut dock_state, vec![Event::PointerMoved(p)], &mut frame);
/// # }
///
/// // The hand is holding "a", pulled out of the bar and back, button still down.
/// assert_eq!(
///     dragged_tab(&ctx, dock_id, &dock_state),
///     Some(TabPath::from((leaf, TabIndex(0))))
/// );
///
/// # run(
/// #     &ctx,
/// #     dock_id,
/// #     &mut dock_state,
/// #     vec![Event::PointerButton {
/// #         pos: home,
/// #         button: PointerButton::Middle,
/// #         pressed: true,
/// #         modifiers: Default::default(),
/// #     }],
/// #     &mut frame,
/// # );
/// # run(
/// #     &ctx,
/// #     dock_id,
/// #     &mut dock_state,
/// #     vec![Event::PointerButton {
/// #         pos: home,
/// #         button: PointerButton::Middle,
/// #         pressed: false,
/// #         modifiers: Default::default(),
/// #     }],
/// #     &mut frame,
/// # );
/// # run(&ctx, dock_id, &mut dock_state, vec![], &mut frame);
///
/// // Middle-click closed it. The hand never let go, but there is nothing left to drag.
/// assert!(dragged_tab(&ctx, dock_id, &dock_state).is_none());
/// ```
pub fn dragged_tab<Tab>(
    ctx: &Context,
    dock_area_id: Id,
    dock_state: &DockState<Tab>,
) -> Option<TabPath> {
    let state = State::load(ctx, dock_area_id);
    // Two questions, deliberately, and this answers the conjunction of them: the *subject* is
    // the field's (`carried_tab` — the one place that says what the hand holds), while `dnd`
    // being open is the *destination* half, and it is what says the drag has been pulled far
    // enough out of the bar for a drop to be resolvable at all.
    //
    // The conjunction is what this function has always meant, back when both halves lived in
    // `dnd` together and the second was implied by the first — and it stays the conjunction now
    // that the two answers exist apart ([`drag_in_flight`] for what is in the hand,
    // [`drag_hover_node`] for where it would land). Kept, rather than split, because it is the
    // one that resolves: a caller asking "which tab is being dragged, as a path into the tree I
    // have" wants both halves and wants them merged with staleness, which is exactly what the
    // two general readers refuse to do for it.
    state.dnd.as_ref()?;
    state.carried_tab()?.resolve(dock_state)
}

/// The node the drag's drop indicator currently prefers, or `None` if no drag is in flight.
///
/// Reads the same per-frame state the dock keeps in `Context` memory, the same way
/// [`dragged_tab`] does — but where that resolves an identity, this does not, and the
/// difference is deliberate.
///
/// [`dragged_tab`] addresses its tab by [`TabId`], which outlives a position and has to be
/// *searched for* to answer "where is it now" — so a stale one and no drag at all look the
/// same from outside, and merging them is correct. A [`NodePath`] is already the node's
/// identity (see [`NodeId`](crate::NodeId)'s docs): there is nothing to search for, only
/// [`DockState::node`](crate::DockState::node) to ask. So this hands back exactly what the
/// dock's own preference currently holds, stale or not, and leaves the liveness question to
/// the caller — which is what lets an outside reader (this crate's own frame-sweep harness,
/// among others) catch the dock holding a preference on a node that is already gone, rather
/// than have that question quietly answered for it.
///
/// In ordinary operation you should never observe a stale value here: the dock clears a dead
/// preference at the top of every frame, before this could be read against it (see FINDINGS.md,
/// "The drop overlay's own preference outlived the node it was pointing at" for the fix, and "A
/// middle click ended egui's drag" for the sibling case on the source side). This read exists
/// for the case where that self-heal is itself under test.
///
/// ```rust
/// # use egui::{
/// #     Atoms, CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
/// # };
/// # use egui_dockyard::{
/// #     DockArea, DockLayout, DockState, NodePath, Style, SurfaceIndex, TabIndex, TabViewer,
/// #     drag_hover_node, tab_widget_id,
/// # };
/// #
/// # struct Viewer;
/// # impl TabViewer for Viewer {
/// #     type Tab = String;
/// #     fn title(&mut self, tab: &String) -> Atoms<'static> {
/// #         Atoms::new(tab.clone())
/// #     }
/// #     fn ui(&mut self, ui: &mut Ui, tab: &String) {
/// #         ui.label(tab.as_str());
/// #     }
/// # }
/// #
/// # fn run(
/// #     ctx: &Context,
/// #     dock_id: Id,
/// #     state: &mut DockState<String>,
/// #     events: Vec<Event>,
/// #     frame: &mut u32,
/// # ) {
/// #     let input = RawInput {
/// #         screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
/// #         time: Some(f64::from(*frame) / 60.0),
/// #         events,
/// #         ..Default::default()
/// #     };
/// #     *frame += 1;
/// #     let mut output = ctx.run_ui(input, |ctx| {
/// #         CentralPanel::default().show(ctx, |ui| {
/// #             DockArea::new(state)
/// #                 .id(dock_id)
/// #                 .style(Style::from_egui(ui.style().as_ref()))
/// #                 .show_inside(ui, &mut Viewer)
/// #                 .apply(ui.ctx(), state, &mut Viewer);
/// #         });
/// #     });
/// #     output.textures_delta.clear();
/// # }
/// #
/// # let dock_id = Id::new("drag_hover_node doctest");
/// # let mut dock_state = DockState::new(vec!["a".to_owned()]);
/// # let root = dock_state.main_surface().root().unwrap();
/// # let [left, right] = dock_state
/// #     .main_surface_mut()
/// #     .split_right(root, 0.5, vec!["b".to_owned()]);
/// # let (left, right) = (
/// #     NodePath::new(SurfaceIndex::main(), left),
/// #     NodePath::new(SurfaceIndex::main(), right),
/// # );
/// # let ctx = Context::default();
/// # let mut frame = 0u32;
/// # run(&ctx, dock_id, &mut dock_state, vec![], &mut frame);
/// #
/// # let tab = dock_state.leaf(left).unwrap().tab_id_at(TabIndex(0)).unwrap();
/// # let home = ctx.read_response(tab_widget_id(dock_id, left, tab)).unwrap().rect.center();
/// # run(&ctx, dock_id, &mut dock_state, vec![Event::PointerMoved(home)], &mut frame);
/// # run(
/// #     &ctx,
/// #     dock_id,
/// #     &mut dock_state,
/// #     vec![Event::PointerButton {
/// #         pos: home,
/// #         button: PointerButton::Primary,
/// #         pressed: true,
/// #         modifiers: Default::default(),
/// #     }],
/// #     &mut frame,
/// # );
/// # let out = home + Vec2::new(0.0, 200.0);
/// # for step in 1..=8u8 {
/// #     let p = home + (out - home) * (f32::from(step) / 8.0);
/// #     run(&ctx, dock_id, &mut dock_state, vec![Event::PointerMoved(p)], &mut frame);
/// # }
///
/// // Still hovering the source's own body — a reorder is a preference too.
/// assert_eq!(drag_hover_node(&ctx, dock_id), Some(left));
///
/// # let target = DockLayout::load(&ctx, dock_id).viewport(right).unwrap().center();
/// # for _ in 0..3 {
/// #     run(&ctx, dock_id, &mut dock_state, vec![Event::PointerMoved(target)], &mut frame);
/// # }
/// // The hand carried "a" onto `right`'s body — the overlay settles a preference on it, and
/// // it names the leaf, not the tab: dropped here, "a" would join "b" rather than replace it.
/// assert_eq!(drag_hover_node(&ctx, dock_id), Some(right));
/// ```
pub fn drag_hover_node(ctx: &Context, dock_area_id: Id) -> Option<NodePath> {
    let (surface, node) = State::load(ctx, dock_area_id)
        .dnd?
        .hover?
        .dst
        .node_address();
    Some(NodePath::new(surface, node?))
}

/// What the dock's hand is holding right now: the one gesture in flight, or `None`.
///
/// Reads the same per-frame state the dock keeps in `Context` memory, the way [`dragged_tab`] and
/// [`drag_hover_node`] do, and it is the general form of the first of them: a carried tab is one
/// [`DragSubject`](crate::DragSubject), a separator and a junction corner are the others. Where [`dragged_tab`]
/// resolves an identity against a tree and merges "stale" with "none", this hands back the
/// gesture as the dock holds it and resolves nothing — the caller decides what to ask of it.
///
/// The liveness question is answered here and not left to the caller, unlike
/// [`drag_hover_node`]: a gesture whose subject stopped existing never gets its `drag_stopped`,
/// so what it leaves in the field is a leftover the dock itself no longer acts on, and reporting
/// it would name a gesture nobody is making. See [`DragInFlight::pass`].
///
/// Same answer as [`DockAreaResponse::dragging`](crate::dock_area::DockAreaResponse::dragging),
/// asked between frames rather than at the end of one — the response is the convenient form for a
/// consumer already reading events; this is the one for an outside driver (automation,
/// diagnostics, this crate's own frame sweep) that has no response in hand.
pub fn drag_in_flight(ctx: &Context, dock_area_id: Id) -> Option<DragInFlight> {
    State::load(ctx, dock_area_id)
        .in_flight_at(ctx.cumulative_pass_nr())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::tab_widget_id;
    use crate::{DockState, NodePath, SurfaceIndex, TabIndex};
    use egui::Id;

    /// Every coordinate of the address has to be part of it. A scheme that drops one hands two
    /// different tabs the same id, and egui answers a press meant for one with the other —
    /// which is exactly the failure this helper exists to keep in one place.
    #[test]
    fn each_coordinate_of_a_tab_address_changes_its_id() {
        let dock_id = Id::new("egui_dockyard::DockArea");
        let dock_state = DockState::new(vec!["a tab", "another tab"]);
        let node = dock_state.main_surface().root().unwrap();

        let main = NodePath::new(SurfaceIndex::main(), node);
        let leaf = dock_state.leaf(main).unwrap();
        let (first, second) = (
            leaf.tab_id_at(TabIndex(0)).unwrap(),
            leaf.tab_id_at(TabIndex(1)).unwrap(),
        );
        let base = tab_widget_id(dock_id, main, first);

        // The same node identity seen through two surfaces: ids are unique within one tree,
        // so the surface has to be part of the address.
        let windowed = NodePath::new(SurfaceIndex::window(0), node);
        assert_ne!(base, tab_widget_id(dock_id, windowed, first));

        // The tab itself.
        assert_ne!(base, tab_widget_id(dock_id, main, second));

        // The dock area itself: two docks in one `Context` must not collide.
        assert_ne!(base, tab_widget_id(Id::new("another dock"), main, first));
    }

    /// The address follows the tab, not its slot in the bar.
    ///
    /// Closing a tab shifts every tab to its right one position down. Under the old,
    /// position-keyed scheme that handed the closed tab's id — and everything egui hangs off an
    /// id: focus, hover, an in-flight drag — to whoever moved into the slot. Under identities
    /// the survivor keeps its own address and the closed tab's address is answered by nobody.
    #[test]
    fn a_tab_keeps_its_address_when_a_neighbour_is_closed() {
        let dock_id = Id::new("egui_dockyard::DockArea");
        let mut dock_state = DockState::new(vec!["first", "second", "third"]);
        let node = dock_state.main_surface().root().unwrap();
        let path = NodePath::new(SurfaceIndex::main(), node);

        let leaf = dock_state.leaf(path).unwrap();
        let closed = tab_widget_id(dock_id, path, leaf.tab_id_at(TabIndex(1)).unwrap());
        let survivor = tab_widget_id(dock_id, path, leaf.tab_id_at(TabIndex(2)).unwrap());

        dock_state
            .leaf_mut(path)
            .unwrap()
            .remove_tab(TabIndex(1))
            .unwrap();

        let leaf = dock_state.leaf(path).unwrap();
        assert_eq!(leaf.len(), 2, "the scene has to have done the removal");
        let now_at_that_slot = tab_widget_id(dock_id, path, leaf.tab_id_at(TabIndex(1)).unwrap());

        assert_eq!(
            now_at_that_slot, survivor,
            "the tab that moved into the vacated slot answers to the address it always had"
        );
        assert_ne!(
            now_at_that_slot, closed,
            "and not to the address of the tab that was closed"
        );
    }
}
