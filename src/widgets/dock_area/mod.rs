/// Due to there being a lot of code to show a dock in a ui every complementing
/// method to ``show`` and ``show_inside`` is put in ``show_extra``.
/// Otherwise ``mod.rs`` would be humongous.
mod show;

// Various components of the `DockArea` which is used when rendering
mod allowed_splits;
mod drag_and_drop;
mod events;
mod state;
mod tab_removal;
mod window_ui;

/// The ids the dock draws its interactive parts under — one source for the scheme.
pub mod ids;

pub use allowed_splits::AllowedSplits;
pub use drag_and_drop::DragSource;
use egui::{Id, Modifiers, emath::*};
pub use events::{DockAreaResponse, DockEvent};
pub use state::{DragInFlight, DragSubject, JunctionArms, WindowEdge};
use tab_removal::TabRemoval;

use crate::layout::DockLayout;
use crate::{NodePath, Style, TabIndex, TabPath, core::DockState};

/// Displays a [`DockState`] in `egui`.
pub struct DockArea<'tree, Tab> {
    id: Id,
    dock_state: &'tree mut DockState<Tab>,
    style: Option<Style>,
    show_add_popup: bool,
    show_add_buttons: bool,
    show_close_buttons: bool,
    tab_context_menus: bool,
    draggable_tabs: bool,
    show_tab_name_on_hover: bool,
    show_window_close_buttons: bool,
    show_window_collapse_buttons: bool,
    show_leaf_close_all_buttons: bool,
    show_leaf_collapse_buttons: bool,
    show_junction_handles: bool,
    show_secondary_button_hint: bool,
    secondary_button_modifiers: Modifiers,
    secondary_button_on_modifier: bool,
    secondary_button_context_menu: bool,
    allowed_splits: AllowedSplits,
    window_bounds: Option<Rect>,

    mutations: Vec<DockMutation>,
    tab_hover_rect: Option<(Rect, TabIndex)>,
    /// Geometry of every node, recomputed by the layout pass each frame.
    ///
    /// Loaded from egui memory at the start of the pass (so the previous frame's values
    /// are available — the tab body needs them to tell whether its viewport moved) and
    /// stored back at the end, where consumers outside the frame can read it via
    /// [`DockLayout::load`].
    layout: DockLayout,
    /// Events accumulated during this render pass, drained into
    /// [`DockAreaResponse::events`] at the end of `show_inside_with_response`.
    events: Vec<DockEvent>,
}

/// A tree edit requested while a [`DockArea`] is drawing.
///
/// Drawing collects these requests and the render epilogue applies them after every surface has
/// been visited. Paths address nodes, so removals and detaches retain their reverse request
/// order when they are applied.
///
/// # The one-frame shift, and how it could be removed
///
/// `Activate` and `SetLeafCollapsed` change what a leaf *shows*, and a leaf draws its tab bar
/// (where the click lands) before its body in the same pass — see `show_leaf`. Deferring them
/// to the epilogue therefore costs one frame: the body of the click frame still paints the
/// previous tab, and the new one appears on the next repaint (~16 ms at 60 fps). This is an
/// accepted behavioural change, not an oversight (decision of 2026-08-26); the click acceptance
/// in the plan covers it.
///
/// It is removable, and the shape is known. The requests are queued *before* the body is drawn,
/// so the body does not need the tree to be mutated — it only needs to know which tab to show.
/// Concretely: let `tab_bar` hand back the activation it just queued, and let `show_leaf` pass
/// that as an override into `tab_body` ("draw this index this frame") instead of `tab_body`
/// reading `leaf.active` off the tree. Same for the collapsed flag, which `show_leaf` already
/// reads once at the top and threads through as a parameter — it would read the pending request
/// first and the node second. The tree still stays read-only for the whole pass; what changes is
/// that draw resolves against *tree + pending queue* rather than the tree alone.
///
/// Not done here on purpose: it widens the seam every drawing site has to thread through, and
/// that widening is only worth paying for once the queue is the *only* way the tree changes —
/// i.e. after the remaining live edits (view state still living inside nodes: tab-bar scroll,
/// window geometry, split fraction) have left the node. Doing it earlier would mean threading
/// the override through code that can still mutate the tree behind it, which buys nothing.
#[derive(Debug, Clone, Copy)]
pub(in crate::widgets::dock_area) enum DockMutation {
    /// Make a tab the active one in its leaf, remembering the previous active tab.
    ///
    /// Applied before `Remove`, which reproduces the present order: activation happens while
    /// drawing, removal in the epilogue. The distinction matters — `remove_tab_choosing` only
    /// asks for a successor when the tab it removes is the active one.
    Activate(TabPath),
    /// Collapse or expand a leaf. Carries the target value rather than a toggle so that two
    /// requests for the same leaf in one frame cannot cancel each other out by ordering.
    SetLeafCollapsed {
        path: NodePath,
        collapsed: bool,
    },
    /// Scroll position of a leaf's tab bar, in points.
    ///
    /// Queued **without** the one-frame shift described above, and that is a property of the
    /// pass rather than of this variant: `tab_bar` reads `scroll` at its top to place the
    /// strip of tabs and only decides the new value at its bottom, once the tabs have been
    /// measured. The write has therefore never affected the frame that made it — the wheel
    /// already showed up one frame later — so moving it into the epilogue changes nothing a
    /// user can see.
    SetLeafScroll {
        path: NodePath,
        scroll: f32,
    },
    /// Where a split cuts its two children, as a fraction of the parent.
    ///
    /// Also free of the one-frame shift, for the same kind of reason: `render_nodes` computes
    /// every rectangle from the stored fractions in its *first* pass and draws separators in
    /// its *third*, so a fraction written by a drag was never read again before the next
    /// frame's layout. Carries the target value, not a delta, so the clamp stays where it is
    /// computed (`nudge_split`) and two requests for one split cannot compound.
    SetSplitFraction {
        path: NodePath,
        fraction: f32,
    },
    Remove(TabRemoval),
    Detach(TabPath),
    Focus(NodePath),
}

// Builder
impl<'tree, Tab> DockArea<'tree, Tab> {
    /// Creates a new [`DockArea`] from the provided [`DockState`].
    #[inline(always)]
    pub fn new(tree: &'tree mut DockState<Tab>) -> DockArea<'tree, Tab> {
        Self {
            id: Id::new("egui_dockyard::DockArea"),
            dock_state: tree,
            style: None,
            show_add_popup: false,
            show_add_buttons: false,
            show_close_buttons: true,
            tab_context_menus: true,
            draggable_tabs: true,
            show_tab_name_on_hover: false,
            allowed_splits: AllowedSplits::default(),
            mutations: Vec::new(),
            tab_hover_rect: None,
            layout: DockLayout::default(),
            events: Vec::new(),
            window_bounds: None,
            show_window_close_buttons: true,
            show_window_collapse_buttons: true,
            show_leaf_close_all_buttons: true,
            show_leaf_collapse_buttons: true,
            show_junction_handles: true,
            show_secondary_button_hint: true,
            secondary_button_modifiers: Modifiers::SHIFT,
            secondary_button_on_modifier: true,
            secondary_button_context_menu: true,
        }
    }

    /// Sets the [`DockArea`] ID. Useful if you have more than one [`DockArea`].
    #[inline(always)]
    pub fn id(mut self, id: Id) -> Self {
        self.id = id;
        self
    }

    /// Sets the look and feel of the [`DockArea`].
    #[inline(always)]
    pub fn style(mut self, style: Style) -> Self {
        self.style = Some(style);
        self
    }

    /// Shows or hides the add button popup.
    /// By default it's `false`.
    pub fn show_add_popup(mut self, show_add_popup: bool) -> Self {
        self.show_add_popup = show_add_popup;
        self
    }

    /// Shows or hides the tab add buttons.
    /// By default it's `false`.
    pub fn show_add_buttons(mut self, show_add_buttons: bool) -> Self {
        self.show_add_buttons = show_add_buttons;
        self
    }

    /// Shows or hides the tab close buttons.
    /// By default it's `true`.
    pub fn show_close_buttons(mut self, show_close_buttons: bool) -> Self {
        self.show_close_buttons = show_close_buttons;
        self
    }

    /// Whether tabs show a context menu when right-clicked.
    /// By default it's `true`.
    pub fn tab_context_menus(mut self, tab_context_menus: bool) -> Self {
        self.tab_context_menus = tab_context_menus;
        self
    }

    /// Whether tabs can be dragged between nodes and reordered on the tab bar.
    /// By default it's `true`.
    pub fn draggable_tabs(mut self, draggable_tabs: bool) -> Self {
        self.draggable_tabs = draggable_tabs;
        self
    }

    /// Whether tabs show their name when hovered over them.
    /// By default it's `false`.
    pub fn show_tab_name_on_hover(mut self, show_tab_name_on_hover: bool) -> Self {
        self.show_tab_name_on_hover = show_tab_name_on_hover;
        self
    }

    /// What directions can a node be split in: left-right, top-bottom, all, or none.
    /// By default it's all.
    pub fn allowed_splits(mut self, allowed_splits: AllowedSplits) -> Self {
        self.allowed_splits = allowed_splits;
        self
    }

    /// Whether a small handle is shown wherever separators meet — a "+" where two dividers
    /// cross (four panels), a "T" where one ends on another (three panels).
    ///
    /// Dragging a handle moves every separator that meets there at once, so the panels around
    /// it are resized in both directions by one gesture. Ctrl+clicking a "+" transposes the
    /// row/column grouping there without moving a pixel on screen — see
    /// [`DockEvent::CrossSplitTransposed`](crate::DockEvent::CrossSplitTransposed), which also
    /// states what that gesture does *not* promise about split ids.
    ///
    /// Not only on a 2x2: a junction is any position one of a split's two children is divided
    /// at, however many parts each of them has.
    ///
    /// By default it's `true`.
    pub fn show_junction_handles(mut self, show_junction_handles: bool) -> Self {
        self.show_junction_handles = show_junction_handles;
        self
    }

    /// Whether tooltip hints are shown for secondary buttons on tab bars.
    /// By default it's `true`.
    pub fn show_secondary_button_hint(mut self, show_secondary_button_hint: bool) -> Self {
        self.show_secondary_button_hint = show_secondary_button_hint;
        self
    }

    /// The key combination used to activate secondary buttons on tab bars.
    /// By default it's [`Modifiers::SHIFT`].
    pub fn secondary_button_modifiers(mut self, secondary_button_modifiers: Modifiers) -> Self {
        self.secondary_button_modifiers = secondary_button_modifiers;
        self
    }

    /// Whether the secondary buttons on tab bars are activated by the modifier key.
    /// By default it's `true`.
    pub fn secondary_button_on_modifier(mut self, secondary_button_on_modifier: bool) -> Self {
        self.secondary_button_on_modifier = secondary_button_on_modifier;
        self
    }

    /// Whether the secondary buttons on tab bars are activated from a context value by right-clicking primary buttons.
    /// By default it's `true`.
    pub fn secondary_button_context_menu(mut self, secondary_button_context_menu: bool) -> Self {
        self.secondary_button_context_menu = secondary_button_context_menu;
        self
    }

    /// The bounds for any windows inside the [`DockArea`]. Defaults to the screen rect.
    /// By default it's set to [`egui::Context::content_rect`].
    #[inline(always)]
    pub fn window_bounds(mut self, bounds: Rect) -> Self {
        self.window_bounds = Some(bounds);
        self
    }

    /// Enables or disables the close button on windows.
    /// By default it's `true`.
    #[inline(always)]
    #[deprecated = "consider using `show_leaf_close_buttons` instead."]
    pub fn show_window_close_buttons(mut self, show_window_close_buttons: bool) -> Self {
        self.show_window_close_buttons = show_window_close_buttons;
        self
    }

    /// Enables or disables the collapsing header on windows.
    /// By default it's `true`.
    #[inline(always)]
    #[deprecated = "consider using `show_leaf_collapse_buttons` instead."]
    pub fn show_window_collapse_buttons(mut self, show_window_collapse_buttons: bool) -> Self {
        self.show_window_collapse_buttons = show_window_collapse_buttons;
        self
    }

    /// Enables or disables the close all tabs button on tab bars.
    /// By default it's `true`.
    #[inline(always)]
    pub fn show_leaf_close_all_buttons(mut self, show_leaf_close_all_buttons: bool) -> Self {
        self.show_leaf_close_all_buttons = show_leaf_close_all_buttons;
        self
    }

    /// Enables or disables the collapse tabs button on tab bars.
    /// By default it's `true`.
    #[inline(always)]
    pub fn show_leaf_collapse_buttons(mut self, show_leaf_collapse_buttons: bool) -> Self {
        self.show_leaf_collapse_buttons = show_leaf_collapse_buttons;
        self
    }
}

impl<Tab> std::fmt::Debug for DockArea<'_, Tab> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockArea").finish_non_exhaustive()
    }
}
