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
use crate::{GapPath, NodePath, Share, Style, SurfaceIndex, TabIndex, TabPath, core::DockState};

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
    collapse_sideways: bool,
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
/// been visited. Between them the tree is read-only, which is the whole point: no drawing site
/// can invalidate a path while sibling surfaces are still being visited.
///
/// # The order they are applied in
///
/// One law, stated once, because "it happened to work" is not an order:
///
/// 1. everything that edits a node **that still exists as drawing saw it** — the transposition,
///    activation, collapsing, minimizing, scroll, fraction — in **request order**, i.e. the
///    order the frame asked for them;
/// 2. `Remove` and `Detach`, in **reverse** request order, because they invalidate paths and the
///    later ones were addressed against a tree the earlier ones had not yet cut;
/// 3. the **last** `Focus` asked for, if any.
///
/// Phase 1 comes before phase 2 for a reason beyond tidiness: a removal asks who inherits the
/// focus only when it takes the *active* tab, so it has to see the activation this frame
/// requested.
///
/// # The one-frame shift, and how it could be removed
///
/// Three of these cost a frame, and the rest cost nothing — which of the two it is follows from
/// where in the pass the value is read, not from what the variant means.
///
/// `Activate`, `SetLeafCollapsed` and `TransposeCross` change what the frame *shows*, and the
/// showing happens after the click that asks: a leaf draws its tab bar (where the click lands)
/// before its body, and the transposition's toggle is drawn while separators below it are still
/// to come. Deferring them therefore costs one frame — the click frame paints the old picture,
/// the new one appears on the next repaint (~16 ms at 60 fps). This is an accepted behavioural
/// change, not an oversight (decision of 2026-08-26); the click acceptance in the plan covers it.
///
/// `SetLeafScroll`, `SetBoundary` and `WindowShown` cost nothing at all, because each is
/// *already* read earlier in the pass than it was written — see the note on each. Deferring them
/// changed no behaviour, and the wheel, separator-drag and window gates say so unchanged.
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
/// Not done here on purpose: it widens the seam every drawing site has to thread through, and it
/// buys back 16 ms on gestures that are single clicks. The precondition it was waiting for is met
/// — as of 2026-08-26 this queue *is* the only way drawing changes the tree — so what is left is
/// a straight trade, to be made when the shift is judged on screen rather than in a plan.
// Not `Copy`: `TransposeCross` carries the measured boundaries of two chains, and there is no
// fixed-size stand-in for "however many parts that chain had". The epilogue reads the list by
// reference, so nothing is cloned to apply it.
#[derive(Debug, Clone)]
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
    /// Put a whole split away behind one arrow, or bring it back — see
    /// [`SplitNode::stowed`](crate::SplitNode::stowed).
    ///
    /// A separate variant rather than [`SetLeafCollapsed`](Self::SetLeafCollapsed) with a split's
    /// path, because it is a different edit on a different kind of node: collapsing is a decision
    /// about one leaf, and stowing is a decision about a subtree that leaves every leaf inside it
    /// alone. `set_leaf_collapsed` panics on a split for exactly that reason.
    ///
    /// Costs the one frame described above, on the same grounds as `SetLeafCollapsed`: the arrow
    /// that asks is drawn on the very bar the answer changes.
    SetSplitStowed {
        path: NodePath,
        stowed: bool,
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
    /// Where one boundary of a row sits, as a proportion of the row's length — see
    /// [`RowNode::set_boundary`](crate::RowNode::set_boundary).
    ///
    /// Also free of the one-frame shift, for the same kind of reason: `render_nodes` computes
    /// every rectangle from the stored weights in its *first* pass and draws separators in
    /// its *third*, so a boundary written by a drag was never read again before the next
    /// frame's layout. Carries the target value, not a delta, so the clamp stays where it is
    /// computed (`nudge_boundary`) and two requests for one gap cannot compound.
    SetBoundary {
        gap: GapPath,
        at: f32,
    },
    /// Every weight of one row at once — see
    /// [`RowNode::set_shares`](crate::RowNode::set_shares).
    ///
    /// The post-image of a drag that moves more than one boundary: a
    /// [`Chain`](crate::SepBehavior::Chain) that pushed past the neighbour that ran out, or a
    /// [`Proportional`](crate::SepBehavior::Proportional) one, which moves every boundary of the
    /// row from the first point. Neither has a single boundary to name, which is what
    /// [`SetBoundary`](Self::SetBoundary) carries — so this is a second variant and not a wider
    /// field on that one: a pair drag still writes the two weights `set_boundary` writes, to the
    /// bit, and the parity of every earlier stage rests on that.
    ///
    /// Free of the one-frame shift for exactly the reason `SetBoundary` is: the weights are read
    /// in the layout pass that has already happened by the time a drag is heard.
    ///
    /// Carries the weights rather than a delta and a mode, for the reason every request here
    /// carries a value: two of them in one frame must not compound, and the clamp stays where it
    /// was computed.
    SetShares {
        row: NodePath,
        shares: Vec<Share>,
    },
    /// Minimize or restore a floating window. Carries the target value, like
    /// [`SetLeafCollapsed`](Self::SetLeafCollapsed) and for the same reason.
    SetWindowMinimized {
        surface: SurfaceIndex,
        minimized: bool,
    },
    /// A floating window has been built this frame out of the one-shot requests recorded in
    /// its [`WindowState`](crate::WindowState) — "move me here", "size me like this", "you are
    /// new" — so those requests are now spent.
    ///
    /// Drawing used to spend them itself, by handing `create_window` a `&mut WindowState` that
    /// took each one out. It reads them now and says here what it read, which is the same
    /// thing one phase later: a window is built exactly once per frame, so nothing else can
    /// observe the difference.
    ///
    /// `took_expanded_height` mirrors the condition under which the height was read — only a
    /// *new* window is resized back to what it was before it collapsed. Carried rather than
    /// re-derived in the epilogue, because by then `SetLeafCollapsed` may have set the flag
    /// again for the very next frame, and clearing the height on that would lose it.
    WindowShown {
        surface: SurfaceIndex,
        took_expanded_height: bool,
    },
    /// Regroup around a crossing, keeping every leaf where it is on screen — the structural edit
    /// behind the toggle drawn where two dividers cross.
    ///
    /// The measured half of it travels in the request, because the tree cannot know it and the
    /// epilogue cannot re-measure it: `bounds` are the boundaries of each of the two chains along
    /// its own axis, and `stack_fraction` the one number from the other axis. Everything about
    /// *which node goes where* is derived from the tree when this is applied, by
    /// [`Tree::transpose_cross`](crate::core::tree::Tree::transpose_cross).
    TransposeCross {
        /// The **gap** the two chains lie on either side of — not the row. A row of three has
        /// two gaps and a crossing sits on exactly one of them; carrying the row would name the
        /// gesture's neighbours only while every row held two.
        outer: GapPath,
        /// Which divider of each chain the crossing is made of.
        at: [usize; 2],
        bounds: [Vec<f32>; 2],
        stack_fraction: f32,
    },
    Remove(TabRemoval),
    Detach(TabPath),
    /// Move the focus to a leaf, because something pointed at it.
    ///
    /// Queued from wherever a gesture *ends* — the click on a tab or in a body, the release of a
    /// window that was moved — and never from its opening press. The focus is part of the tree a
    /// consumer saves, so moving it announces a
    /// [`DockEvent::LayoutCommitted`](events::DockEvent::LayoutCommitted); asking for it while a
    /// gesture is still live would announce a change per frame of that gesture, which is exactly
    /// what the `dst` sweep's commit rule is written against.
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
            collapse_sideways: false,
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

    /// Lets a collapsed leaf hide **sideways**, into a narrow vertical strip against one edge
    /// of its split, instead of keeping its whole column. By default it's `false`.
    ///
    /// **Experimental.** Collapsing has always meant "give up your body and be a tab bar",
    /// which spends *height* — so under a horizontal split there was nobody to give the height
    /// to, and a collapsed leaf kept its column rather than leave a hole belonging to no node.
    /// With this on, such a leaf gives up its *width* instead: it shrinks to a strip with an
    /// expand arrow, and the sibling column immediately takes the width, so no hole appears.
    ///
    /// The direction is not stored anywhere — it is read off the parent split, so a leaf
    /// dragged into a vertical split goes back to collapsing into a row, and transposing a
    /// split turns its strips the other way by itself. Nothing new is serialized either:
    /// turning this back off restores the old layout with no migration of saved trees.
    ///
    /// Only a collapsed *leaf* whose sibling is open collapses sideways. Two collapsed
    /// siblings keep their columns (there would be nobody to take the width), and so does a
    /// collapsed *split*, whose subtree is rows of tab bars that do not fit in a strip.
    ///
    /// # Putting a whole side away
    ///
    /// The last of those is the case that turns up in practice — a side made of several leaves,
    /// where collapsing them one by one frees no space at all. So this knob also enables the
    /// gesture for the other spelling: **the secondary-button modifier plus the collapse arrow
    /// of any leaf in a side puts that whole side away as a unit** (see
    /// [`SplitNode::stowed`](crate::SplitNode::stowed)). The side becomes one strip with one
    /// arrow, its insides are left exactly as they are for when it comes back, and the arrow on
    /// the strip brings it back.
    ///
    /// "The side" is the child of the root the leaf belongs to — not the leaf's parent — so the
    /// gesture means the same thing from any leaf in it, however deeply the side is split
    /// inside. On a leaf that is *itself* a side the modifier adds nothing: the plain arrow
    /// already folds such a leaf into a strip, which is the same picture.
    ///
    /// Behind this knob because the layout is: with it off, a side stowed under a horizontal
    /// split would draw one bar and leave the rest of its column belonging to no node — the very
    /// hole described above. And on a *floating* surface the same modifier already means
    /// "collapse the whole window", which keeps its meaning, so the gesture is one for the main
    /// surface unless [`secondary_button_on_modifier`](Self::secondary_button_on_modifier) is
    /// off.
    ///
    /// Stowing is the one thing here that *is* serialized, because it is a decision rather than
    /// something derived from the leaves; a tree saved before it existed loads as not stowed,
    /// which is what it was.
    #[inline(always)]
    pub fn collapse_sideways(mut self, collapse_sideways: bool) -> Self {
        self.collapse_sideways = collapse_sideways;
        self
    }
}

impl<Tab> std::fmt::Debug for DockArea<'_, Tab> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockArea").finish_non_exhaustive()
    }
}
