use egui::{Atoms, Id, Ui};

use crate::{NodePath, TabStyle};

/// Defines how a tab should behave and be rendered inside a [`Tree`](crate::Tree).
pub trait TabViewer {
    /// The type of tab in which you can store state to be drawn in your tabs.
    type Tab;

    /// The title to be displayed in the tab bar.
    ///
    /// A title is a row of [`Atoms`]: text, an image, or both — `Atoms::new("Name")` for a plain
    /// name, `Atoms::new((icon, "Name"))` for a name behind an icon. The dock lays them out left
    /// to right, and the order is what decides what a squeezed tab keeps: an icon put first is
    /// the last thing to go, which is the behaviour a browser's favicon has.
    ///
    /// # Lifetime
    ///
    /// `'static`, because a title outlives the call that produced it: the dock collects every
    /// title in a bar (or in a side strip, which names the tabs of a whole subtree) *before* it
    /// draws any of them — the widths have to be shared out before the first tab is placed — and
    /// a title borrowing the viewer could not survive the call that asks the next tab for its
    /// own. An image referring to a file or a texture is `'static` already
    /// ([`egui::include_image!`], [`egui::Image::from_texture`]); a URI held in a field is
    /// carried across by cloning the string.
    fn title(&mut self, tab: &Self::Tab) -> Atoms<'static>;

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
    /// If not implemented, uses tab title text as an ID source — the text of the title's atoms,
    /// or the `alt_text` of its first image when it carries no text at all. A title made of an
    /// icon with neither is the one case this default cannot tell apart from another like it, so
    /// implement this yourself when your tabs are icons alone.
    fn id(&mut self, tab: &Self::Tab) -> Id {
        Id::new(self.title(tab).text())
    }

    /// Called after each tab button is shown, so you can add a tooltip, check for clicks, etc.
    fn on_tab_button(&mut self, _tab: &Self::Tab, _response: &egui::Response) {}

    /// Returns `true` if the user of your app should be able to close a given `_tab`.
    ///
    /// Asked while the bar is drawn, and it decides what is *shown*: a tab answering `false`
    /// gets no close button, is not closed by a middle click, and disables the buttons that
    /// close its whole leaf or window. Whether a close that was nonetheless asked for is
    /// carried out is a separate question, and one the tree cannot be asked while it is being
    /// drawn — see [`DockDraw::settle_closes`](crate::DockDraw::settle_closes).
    ///
    /// By default, `true` is always returned.
    fn is_closeable(&self, _tab: &Self::Tab) -> bool {
        true
    }

    /// This is called every frame after [`ui`](Self::ui) is called, if the `_tab` is active.
    ///
    /// Returns `true` if the tab should be forced to close, `false` otherwise.
    ///
    /// The close is marked [`ForcedRemoval`](crate::ForcedRemoval), so an application settling
    /// its closes can tell this one — which it asked for itself — from a hand on a button.
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
