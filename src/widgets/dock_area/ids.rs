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

use super::state::State;
use crate::{DockState, NodePath, TabId, TabPath};

/// Id of the widget one tab is drawn as, in the dock area with id `dock_area_id`.
///
/// `dock_area_id` is the id of the [`DockArea`](crate::DockArea) — the default is
/// `Id::new("egui_dock::DockArea")`, or whatever was passed to
/// [`DockArea::id`](crate::DockArea::id).
///
/// `tab` is the tab's identity inside its leaf, which
/// [`LeafNode::tab_id_at`](crate::LeafNode::tab_id_at) resolves from a position.
///
/// ```rust
/// # use egui_dock::{DockState, NodePath, SurfaceIndex, TabIndex, tab_widget_id};
/// # egui::__run_test_ctx(|ctx| {
/// let dock_state = DockState::new(vec!["a tab"]);
/// let dock_id = egui::Id::new("egui_dock::DockArea");
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
/// #     CentralPanel, Context, Event, Id, PointerButton, Pos2, RawInput, Rect, Ui, Vec2,
/// #     WidgetText,
/// # };
/// # use egui_dock::{
/// #     DockArea, DockState, NodePath, Style, SurfaceIndex, TabIndex, TabPath, TabViewer,
/// #     dragged_tab, tab_widget_id,
/// # };
/// #
/// # struct Viewer;
/// # impl TabViewer for Viewer {
/// #     type Tab = String;
/// #     fn title(&mut self, tab: &mut String) -> WidgetText {
/// #         tab.as_str().into()
/// #     }
/// #     fn ui(&mut self, ui: &mut Ui, tab: &mut String) {
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
/// #                 .show_inside(ui, &mut Viewer);
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
    State::load(ctx, dock_area_id)
        .dnd?
        .drag
        .src
        .resolve(dock_state)
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
        let dock_id = Id::new("egui_dock::DockArea");
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
        let dock_id = Id::new("egui_dock::DockArea");
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
