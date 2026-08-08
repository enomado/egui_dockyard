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
//! The id is an address, not a promise about layout: it stays stable as long as the node
//! identity and the tab position do.

use egui::Id;

use crate::{NodePath, TabIndex};

/// Id of the widget one tab is drawn as, in the dock area with id `dock_area_id`.
///
/// `dock_area_id` is the id of the [`DockArea`](crate::DockArea) — the default is
/// `Id::new("egui_dock::DockArea")`, or whatever was passed to
/// [`DockArea::id`](crate::DockArea::id).
///
/// ```rust
/// # use egui_dock::{DockState, NodePath, SurfaceIndex, TabIndex, tab_widget_id};
/// # egui::__run_test_ctx(|ctx| {
/// let dock_state = DockState::new(vec!["a tab"]);
/// let dock_id = egui::Id::new("egui_dock::DockArea");
/// let leaf = dock_state.main_surface().root().unwrap();
/// let path = NodePath::new(SurfaceIndex::main(), leaf);
/// // What the dock drew that tab as — `ctx.read_response` answers where it ended up.
/// let _id = tab_widget_id(dock_id, path, TabIndex(0));
/// # });
/// ```
pub fn tab_widget_id(dock_area_id: Id, path: NodePath, tab: TabIndex) -> Id {
    dock_area_id
        .with((path.surface, "surface"))
        .with((path.node, "node"))
        .with((tab.0, "tab"))
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
        let dock_state = DockState::new(vec!["a tab"]);
        let node = dock_state.main_surface().root().unwrap();

        let main = NodePath::new(SurfaceIndex::main(), node);
        let base = tab_widget_id(dock_id, main, TabIndex(0));

        // The same node identity seen through two surfaces: ids are unique within one tree,
        // so the surface has to be part of the address.
        let windowed = NodePath::new(SurfaceIndex::window(0), node);
        assert_ne!(base, tab_widget_id(dock_id, windowed, TabIndex(0)));

        // The tab position.
        assert_ne!(base, tab_widget_id(dock_id, main, TabIndex(1)));

        // The dock area itself: two docks in one `Context` must not collide.
        assert_ne!(
            base,
            tab_widget_id(Id::new("another dock"), main, TabIndex(0))
        );
    }
}
