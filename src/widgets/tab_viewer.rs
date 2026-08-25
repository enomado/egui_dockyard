use egui::{Id, Ui, WidgetText};

use crate::{LeafNode, NodePath, TabId, TabIndex, TabStyle};

/// Defines how a tab should behave and be rendered inside a [`Tree`](crate::Tree).
pub trait TabViewer {
    /// The type of tab in which you can store state to be drawn in your tabs.
    type Tab;

    /// The title to be displayed in the tab bar.
    fn title(&mut self, tab: &Self::Tab) -> WidgetText;

    /// Actual tab content.
    ///
    /// The dock tree is read-only for the whole draw pass. Return application-owned actions
    /// from the viewer instead of mutating this tab in place; the owner applies them afterwards.
    fn ui(&mut self, ui: &mut Ui, tab: &Self::Tab);

    /// Content inside the context menu shown when the tab is right-clicked.
    ///
    /// `_path` specifies which [`SurfaceIndex`](crate::SurfaceIndex) and [`Node`](crate::Node)
    /// that this particular context menu belongs to.
    fn context_menu(&mut self, _ui: &mut Ui, _tab: &Self::Tab, _path: NodePath) {}

    /// Unique ID for this tab.
    ///
    /// If not implemented, uses tab title text as an ID source.
    fn id(&mut self, tab: &Self::Tab) -> Id {
        Id::new(self.title(tab).text())
    }

    /// Called after each tab button is shown, so you can add a tooltip, check for clicks, etc.
    fn on_tab_button(&mut self, _tab: &Self::Tab, _response: &egui::Response) {}

    /// This is called when the `_tab` gets closed by the user.
    ///
    /// Returns an `OnCloseResponse` which determines what happens to the tab after this function gets called.
    fn on_close(&mut self, _tab: &Self::Tab) -> OnCloseResponse {
        OnCloseResponse::Close
    }

    /// Returns `true` if the user of your app should be able to close a given `_tab`.
    ///
    /// By default, `true` is always returned.
    fn is_closeable(&self, _tab: &Self::Tab) -> bool {
        true
    }

    /// Which tab should take the focus when the **active** tab at `_closing` is closed.
    ///
    /// Return `None` — the default — to let the dock decide, which means its focus history:
    /// the tab you came from, then the one before that, and the left neighbour only once that
    /// runs out. Override this when the application knows better than a history can: a tab
    /// that owns the one being closed, a pinned tab that should always be landed on, an order
    /// that is the application's own rather than the order of visits.
    ///
    /// Called only when the closed tab is the active one; closing a tab nobody is looking at
    /// does not move the focus. `_leaf` is the leaf as it stands **before** the removal, so
    /// [`LeafNode::history_ids`] is available to consult (or to ignore) and `_leaf[_closing]`
    /// is the tab on its way out.
    ///
    /// # Panics
    ///
    /// The returned identity has to be a tab of `_leaf` other than the one being closed. A
    /// successor that will not be there when the removal is done is not an answer, and the
    /// dock says so rather than quietly falling back.
    fn successor_on_close(
        &mut self,
        _leaf: &LeafNode<Self::Tab>,
        _closing: TabIndex,
    ) -> Option<TabId> {
        None
    }

    /// Returns `true` if the user of your app should be able to close a given `_tab`.
    ///
    /// By default, `true` is always returned.
    #[deprecated = "Use the `TabViewer::is_closeable` function instead."]
    fn closeable(&mut self, _tab: &mut Self::Tab) -> bool {
        true
    }

    /// This is called every frame after [`ui`](Self::ui) is called, if the `_tab` is active.
    ///
    /// Returns `true` if the tab should be forced to close, `false` otherwise.
    ///
    /// In the event this function returns true the tab will be removed without calling `on_close`.
    fn force_close(&mut self, _tab: &Self::Tab) -> bool {
        false
    }

    /// This is called when the add button is pressed.
    ///
    /// `_path` specifies which [`SurfaceIndex`](crate::SurfaceIndex) and on which
    /// [`Node`](crate::Node) this particular add button was pressed.
    fn on_add(&mut self, _path: NodePath) {}

    /// Called when the rectangle of the tab content changes.
    ///
    /// This can happen when the window is resized, panels are docked or undocked,
    /// or when the layout of the dock area is changed in any way that affects
    /// the available space for the tab content.
    ///
    /// This is useful for tabs that need to adjust their content based on the
    /// available space.
    fn on_rect_changed(&mut self, _tab: &Self::Tab) {}

    /// Content of the popup under the add button. Useful for selecting what type of tab to add.
    ///
    /// This requires that [`DockArea::show_add_buttons`](crate::DockArea::show_add_buttons) and
    /// [`DockArea::show_add_popup`](crate::DockArea::show_add_popup) are set to `true`.
    fn add_popup(&mut self, _ui: &mut Ui, _path: NodePath) {}

    /// Sets custom style for given tab.
    fn tab_style_override(&self, _tab: &Self::Tab, _global_style: &TabStyle) -> Option<TabStyle> {
        None
    }

    /// Specifies a tab's ability to be shown in a window.
    ///
    /// Returns `false` if this tab should never be turned into a window.
    fn allowed_in_windows(&self, _tab: &Self::Tab) -> bool {
        true
    }

    /// Whether the tab body will be cleared with the color specified in
    /// [`TabBarStyle::bg_fill`](crate::TabBarStyle::bg_fill).
    fn clear_background(&self, _tab: &Self::Tab) -> bool {
        true
    }

    /// Returns `true` if the horizontal and vertical scroll bars will be shown for `tab`.
    ///
    /// By default, both scroll bars are shown.
    fn scroll_bars(&self, _tab: &Self::Tab) -> [bool; 2] {
        [true, true]
    }
}

/// Determines what happens to a tab when a user attempts to close it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OnCloseResponse {
    /// Closes the tab.
    Close,
    /// Focuses on the tab.
    Focus,
    /// Ignores the close request.
    Ignore,
}
