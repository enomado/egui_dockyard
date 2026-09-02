use std::ops::BitOrAssign;

use egui::{
    Context, Id, LayerId, NumExt, Order, Painter, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2,
    emath::{GuiRounding, inverse_lerp},
    vec2,
};

use crate::{
    AllowedSplits, DockState, NodeId, NodePath, Split, Style, SurfaceIndex, TabDestination, TabId,
    TabInsert, TabPath,
};

#[derive(Debug, Clone)]
pub(super) struct HoverData {
    /// Rect of the hovered element.
    pub rect: Rect,

    /// The "address" of the tab/node being hovered over.
    pub dst: TreeComponent,

    /// If a tab title or the tab head is hovered, this is the rect of it.
    pub tab: Option<Rect>,
}

/// The tab a drag is carrying, addressed by **identity**.
///
/// # Why not a [`TabPath`]
///
/// A drag outlives the frame it started in, and the leaf it came from can be edited while
/// it is in flight: the dragged tab itself can be closed (middle-click closes a tab, and the
/// dragged tab is still a tab in the bar), a neighbour can be closed, or the application can
/// rewrite the `DockState` between frames. `TabIndex` is a *position*, and every one of those
/// edits renumbers positions — so a carried `TabPath` either names a different tab than the
/// hand grabbed, or names nothing at all and panics the next time it is indexed with.
///
/// The identity does neither: [`TabId`] is handed out per leaf and never reused, so
/// [`resolve`](Self::resolve) answers "where is it now", and answers `None` exactly when the
/// tab is gone — which is the one thing the drag has to notice, since a drag of a tab that no
/// longer exists is over.
///
/// Public because a carried tab is published: it is what
/// [`DragSubject::Tab`](super::DragSubject::Tab) holds, and a reader that got the address but not
/// the operation over it would have to write [`resolve`](Self::resolve) again at its own call
/// site — which is the second place this type exists to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragSource {
    /// The surface the tab was picked up from.
    pub surface: SurfaceIndex,
    /// The leaf it came out of. An identity, so an edit of the surface does not rename it.
    pub node: NodeId,
    /// The tab itself, by the identity its leaf handed out — never by a position in the bar.
    pub tab: TabId,
}

impl DragSource {
    /// Where the dragged tab sits now, or `None` if it has left the tree.
    pub fn resolve<Tab>(&self, dock_state: &DockState<Tab>) -> Option<TabPath> {
        let leaf = dock_state
            .leaf(NodePath::new(self.surface, self.node))
            .ok()?;
        let tab = leaf.index_of(self.tab)?;
        Some(TabPath::new(self.surface, self.node, tab))
    }

    /// The leaf the tab was picked up from, as one address.
    pub fn node_path(&self) -> NodePath {
        NodePath::new(self.surface, self.node)
    }
}

#[derive(Debug, Clone)]
pub(super) enum TreeComponent {
    Surface(SurfaceIndex),
    Node(NodePath),
    Tab(TabPath),
}

impl TreeComponent {
    pub(super) fn as_tab_destination(&self) -> TabDestination {
        match *self {
            TreeComponent::Surface(surface) => TabDestination::EmptySurface(surface),
            TreeComponent::Node(dst) => TabDestination::Node(dst, TabInsert::Append),
            TreeComponent::Tab(dst) => {
                TabDestination::Node(dst.node_path(), TabInsert::Insert(dst.tab))
            }
        }
    }

    pub(super) fn node_address(&self) -> (SurfaceIndex, Option<NodeId>) {
        match *self {
            TreeComponent::Surface(surface) => (surface, None),
            TreeComponent::Node(dst) => (dst.surface, Some(dst.node)),
            TreeComponent::Tab(dst) => (dst.surface, Some(dst.node)),
        }
    }

    pub(super) fn surface_address(&self) -> SurfaceIndex {
        match *self {
            TreeComponent::Surface(surface)
            | TreeComponent::Node(NodePath { surface, .. })
            | TreeComponent::Tab(TabPath { surface, .. }) => surface,
        }
    }

    pub(super) fn is_surface(&self) -> bool {
        matches!(self, TreeComponent::Surface(_))
    }

    /// Whether the node this addresses has left its tree since this was recorded.
    ///
    /// [`Surface`](Self::Surface) addresses no node and can never go stale this way: an empty
    /// surface is a legitimate, permanent drop target, not a reference that decays. A
    /// [`Node`](Self::Node) or [`Tab`](Self::Tab) destination does decay, the same way a drag's
    /// source does — see [`DragSource::resolve`] — except there is nothing to *resolve*: a
    /// [`NodeId`] already is the node's identity, so the question is answered by
    /// [`DockState::node`] directly, not by searching for where the node "is now".
    pub(super) fn node_is_gone<Tab>(&self, dock_state: &DockState<Tab>) -> bool {
        match self.node_address() {
            (surface, Some(node)) => dock_state.node(NodePath::new(surface, node)).is_err(),
            (_, None) => false,
        }
    }
}

/// The layer every drop overlay shape is painted into.
///
/// # Why the layer is an area, and why it is registered every pass
///
/// This used to be `LayerId::new(Order::Foreground, Id::new("overlay"))` and nothing else, which
/// is the shape the junction handle had and had fixed for the same reason. egui answers "who is on
/// top" twice, by two rules: the pointer is ranked by `Areas::compare_order`, which resolves a tie
/// within a tier through `order_map` — a layer that never joined it compares below every area of
/// the tier, since `None < Some(i)`; the paint is `GraphicLayers::drain`, which walks a tier's
/// areas in that order and *then* sweeps up every layer of the tier it has not seen. So a layer
/// that is not an area is painted **last**, over menus and popups, however the pointer resolved.
/// Showing an [`egui::Area`] is what calls `Areas::set_state` and so what puts a layer in that
/// list; nothing short of one does.
///
/// **The rank is the order of first appearance, and that is why this is registered on every pass
/// and not only while a drag is in flight.** `Areas::set_state` appends a layer the first time it
/// is seen and never re-sorts, and [`egui::Area`] additionally moves itself to the *top* whenever
/// it was not visible last frame. An overlay area created when a drag starts would therefore be
/// pushed above every menu already open — the same bug in a new place, and worse for appearing
/// only sometimes. Registered from the dock's own pass, it takes its rank once, before any menu
/// the application opens later, and keeps it. The area stays empty: it is here to be *ranked*, and
/// the shapes go into its layer through the painters below.
///
/// Only the *paint* is at stake. The overlay asks egui for no widget at all — the drop buttons are
/// hit-tested against [`DragDropState::pointer`] arithmetically — so there is no press here for a
/// menu to win or lose, which is why `sense(Sense::hover())` is enough and why this has one test
/// scene rather than the junction handle's two.
pub(super) fn register_overlay_layer(ui: &Ui, dock_id: Id) {
    egui::Area::new(overlay_id(dock_id))
        .order(Order::Foreground)
        .fixed_pos(Pos2::ZERO)
        .movable(false)
        .sense(egui::Sense::hover())
        .constrain(false)
        .show(ui.ctx(), |_| {});
}

/// The overlay's layer, named the same way whether or not anything is being dragged.
pub(super) fn overlay_layer(dock_id: Id) -> LayerId {
    LayerId::new(Order::Foreground, overlay_id(dock_id))
}

/// Scoped to the dock: this was a bare `Id::new("overlay")`, which two [`DockArea`](crate::DockArea)s
/// in one application shared. Sharing a *painter's* layer was untidy; sharing an **area** id is a
/// clash egui reports, so the id follows the dock that owns the drag.
fn overlay_id(dock_id: Id) -> Id {
    dock_id.with("overlay")
}

fn make_overlay_painter(ui: &Ui, overlay: LayerId) -> Painter {
    ui.ctx().layer_painter(overlay)
}

fn draw_highlight_rect(rect: Rect, ui: &Ui, overlay: LayerId, style: &Style) {
    let painter = make_overlay_painter(ui, overlay);
    painter.rect(
        rect.expand(style.overlay.hovered_leaf_highlight.expansion),
        style.overlay.hovered_leaf_highlight.corner_radius,
        style.overlay.hovered_leaf_highlight.color,
        style.overlay.hovered_leaf_highlight.stroke,
        StrokeKind::Inside,
    );
}

// Draws one of the Tab drop destination icons inside `rect`, which one you get is specified by `is_top_bottom`.
fn button_ui(
    rect: Rect,
    ui: &Ui,
    overlay: LayerId,
    lock: &mut bool,
    mouse_pos: Pos2,
    style: &Style,
    split: Option<Split>,
) -> bool {
    let visuals = &style.overlay;
    let button_stroke = Stroke::new(1.0_f32, visuals.button_color);
    let painter = make_overlay_painter(ui, overlay);
    painter.rect_stroke(rect, 0.0, visuals.button_border_stroke, StrokeKind::Inside);
    let rect = rect.shrink(rect.width() * 0.1);
    painter.rect_stroke(rect, 0.0, button_stroke, StrokeKind::Inside);
    let rim = { Rect::from_two_pos(rect.min, rect.lerp_inside(vec2(1.0, 0.1))) };
    painter.rect(
        rim,
        0.0,
        visuals.button_color,
        Stroke::NONE,
        StrokeKind::Inside,
    );

    if let Some(split) = split {
        for line in DASHED_LINE_ALPHAS.chunks(2) {
            let start = rect.lerp_inside(lerp_vec(split, line[0]));
            let end = rect.lerp_inside(lerp_vec(split, line[1]));
            painter.line_segment([start, end], button_stroke);
        }
    }
    let is_mouse_over = rect
        .expand(style.overlay.feel.interact_expansion)
        .contains(mouse_pos);
    if is_mouse_over && !*lock {
        let vertical_alphas = vec2(1.0, 0.5);
        let horizontal_alphas = vec2(0.5, 1.0);
        let rect = match split {
            Some(Split::Above) => Rect::from_min_size(rect.min, rect.size() * vertical_alphas),
            Some(Split::Left) => Rect::from_min_size(rect.min, rect.size() * horizontal_alphas),
            Some(Split::Below) => {
                let min = rect.lerp_inside(lerp_vec(Split::Below, 0.0));
                Rect::from_min_size(min, rect.size() * vertical_alphas)
            }
            Some(Split::Right) => {
                let min = rect.lerp_inside(lerp_vec(Split::Right, 0.0));
                Rect::from_min_size(min, rect.size() * horizontal_alphas)
            }
            _ => rect,
        };
        painter.rect_filled(rect, 0.0, style.overlay.selection_color);
    }
    lock.bitor_assign(is_mouse_over);
    is_mouse_over
}

const DASHED_LINE_ALPHAS: [f32; 8] = [
    0.0625, 0.1875, 0.3125, 0.4375, 0.5625, 0.6875, 0.8125, 0.9375,
];

#[derive(PartialEq, Eq)]
enum LockState {
    /// Lock is unlocked.
    Unlocked,

    /// Lock remains locked, but can be unlocked.
    SoftLock,

    /// Lock is locked forever.
    HardLock,
}

#[derive(Debug, Clone)]
pub(super) struct DragDropState {
    /// The node the overlay currently prefers as a drop target, or `None` if nothing live is
    /// under the hand right now.
    ///
    /// Distinct from `dnd` being absent entirely (see [`State::set_drag_and_drop`]): a drag can
    /// be open with genuinely nowhere to drop — mid-flight over a gap, or right after the node
    /// it had settled on left the tree (see [`Self::drop_stale_hover`]) — and that is a fact
    /// about the *destination*, not about whether a drag exists at all. `.drag` below is what
    /// answers that second question.
    ///
    /// [`State::set_drag_and_drop`]: super::state::State::set_drag_and_drop
    pub hover: Option<HoverData>,

    /// The rectangle of the leaf the carried tab came from, as it stood when the drag was
    /// published — the size a "drop this into a window" preview is drawn at.
    ///
    /// Geometry and nothing else. *What* is being carried is not here and is not duplicated
    /// here: it is [`State::carried_tab`](super::state::State::carried_tab), the one place that
    /// says what the hand holds. This struct is the destination half of a drag.
    pub source_rect: Rect,

    pub pointer: Pos2,
    /// Is some when the pointer is over rect, f64 holds the time when the lock was last active.
    pub locked: Option<f64>,
}

/// One frame's aim: where the carried tab is pointed, what is in the hand, and what the dock
/// will let it do there.
///
/// The two resolvers — [`resolve_traditional`](DragDropState::resolve_traditional) and
/// [`resolve_icon_based`](DragDropState::resolve_icon_based) — differ in what they *draw*, not in
/// what they need to know, and they took the same eight arguments in the same order at the same
/// two call sites. That is one thing with a name, so it has one: the caller settles what a drop
/// would mean this frame, and the overlay the style asked for is handed it.
///
/// `Copy`, so each resolver destructures it once at the top and reads the same names it always
/// did.
#[derive(Clone, Copy)]
pub(super) struct DropAim<'a> {
    /// Where the pointer settled, as `State::set_drag_and_drop` wrote it this frame.
    pub hover: &'a HoverData,
    /// What the hand holds, by identity — the source half of the drag.
    pub source: DragSource,
    /// The `Ui` the overlay is painted through.
    pub ui: &'a Ui,
    /// The dock's own foreground layer, named once per pass so its rank among the other
    /// foreground areas is taken before the application opens anything later.
    pub overlay: LayerId,
    /// The style the overlay is drawn in — the faded one while a window is fading.
    pub style: &'a Style,
    /// Which splits *this* destination can house: the dock's own setting, narrowed by what the
    /// hovered node allows (a surface and a leaf being deserted can house none).
    pub allowed_splits: AllowedSplits,
    /// Whether the carried tab is allowed to become a window at all — the consumer's answer,
    /// asked of the tab itself.
    pub windows_allowed: bool,
    /// The area a window preview is kept inside.
    pub window_bounds: Rect,
}

impl DragDropState {
    /// Drops a stale destination without touching `.drag`.
    ///
    /// Called when `.hover` has been found to address a node that is already gone. Earlier this
    /// cleared the whole `DragDropState` — but the drag's *source* had not gone stale, only the
    /// destination had, and a DST sweep caught the consequence: `ctx.dragged_id()` and the
    /// dock's own `dragged_tab` disagreeing for a frame, because clearing `dnd` entirely ended a
    /// drag that was still live everywhere else. This clears (and unlocks) only the half that
    /// actually decayed; `State::set_drag_and_drop` treats `hover: None` as nothing to hold onto
    /// and writes a fresh one the moment a live one arrives, same as it always has for a `dnd`
    /// that does not exist yet.
    pub(super) fn drop_stale_hover(&mut self) {
        self.hover = None;
        self.locked = None;
    }

    pub(super) fn resolve_icon_based(&mut self, aim: DropAim<'_>) -> Option<TabDestination> {
        let DropAim {
            hover,
            source,
            ui,
            overlay,
            style,
            allowed_splits,
            windows_allowed,
            window_bounds,
        } = aim;
        assert!(hover.tab.is_none());

        draw_highlight_rect(hover.rect, ui, overlay, style);
        let mut hovering_buttons = false;
        let total_button_spacing = style.overlay.button_spacing * 2.0;
        let (rect, pointer) = (hover.rect, self.pointer);
        let rect = rect.shrink(style.overlay.button_spacing);
        let shortest_side = ((rect.width() - total_button_spacing) / 3.0)
            .min((rect.height() - total_button_spacing) / 3.0)
            .min(style.overlay.max_button_size);

        let mut destination: Option<TabDestination> = windows_allowed.then(|| {
            TabDestination::Window(Rect::from_min_size(pointer, self.source_rect.size()).into())
        });

        let center = rect.center();
        let rect = Rect::from_center_size(center, Vec2::splat(shortest_side));

        if button_ui(
            rect,
            ui,
            overlay,
            &mut hovering_buttons,
            pointer,
            style,
            None,
        ) {
            match hover.dst {
                TreeComponent::Node(path) => {
                    destination = Some(TabDestination::Node(path, TabInsert::Append))
                }
                TreeComponent::Surface(surface) => {
                    destination = Some(TabDestination::EmptySurface(surface))
                }
                _ => (),
            }
        }

        for split in [Split::Below, Split::Right, Split::Above, Split::Left] {
            match allowed_splits {
                AllowedSplits::TopBottomOnly if !split.is_top_bottom() => continue,
                AllowedSplits::LeftRightOnly if !split.is_left_right() => continue,
                AllowedSplits::None => continue,
                _ => {
                    let offset_value = shortest_side + style.overlay.button_spacing;
                    let offset_vector = match split {
                        Split::Above => vec2(0.0, -offset_value),
                        Split::Below => vec2(0.0, offset_value),
                        Split::Left => vec2(-offset_value, 0.0),
                        Split::Right => vec2(offset_value, 0.0),
                    };
                    if button_ui(
                        Rect::from_center_size(center + offset_vector, Vec2::splat(shortest_side)),
                        ui,
                        overlay,
                        &mut hovering_buttons,
                        pointer,
                        style,
                        Some(split),
                    ) && let TreeComponent::Node(path) = hover.dst
                    {
                        destination = Some(TabDestination::Node(path, TabInsert::Split(split)))
                    }
                }
            }
        }
        let hovering_rect = hover.rect.contains(pointer);
        let target_lock_state = match (hovering_rect, hovering_buttons) {
            (false, false) => LockState::Unlocked,
            (_, true) => LockState::HardLock,
            (true, _) => LockState::SoftLock,
        };
        self.update_lock(hover, target_lock_state, style, ui.ctx());
        if let Some(TabDestination::Window(rect)) = destination {
            let rect = self.window_preview_rect(source, rect.into());
            let rect_bounded = constrain_rect_to_area(ui, rect, window_bounds);
            draw_window_rect(rect_bounded, ui, overlay, style);
        }
        destination
    }

    pub(super) fn resolve_traditional(&mut self, aim: DropAim<'_>) -> Option<TabDestination> {
        let DropAim {
            hover,
            source,
            ui,
            overlay,
            style,
            allowed_splits,
            windows_allowed,
            window_bounds,
        } = aim;
        // If windows are not allowed, any hover over a window is immediately disallowed.
        if !windows_allowed && hover.dst.surface_address() != SurfaceIndex::main() {
            return None;
        }
        draw_highlight_rect(hover.rect, ui, overlay, style);

        // Deals with hovers over tab bar and tab titles.
        if let Some(rect) = hover.tab {
            draw_drop_rect(rect, ui, overlay, style);
            let target_lock_state = if rect.contains(self.pointer) {
                LockState::SoftLock
            } else {
                LockState::Unlocked
            };
            self.update_lock(hover, target_lock_state, style, ui.ctx());
            return Some(hover.dst.as_tab_destination());
        }

        // Main cases, splits, window creations, etc.
        let (hover_rect, pointer) = (hover.rect, self.pointer);
        let center = hover_rect.center();

        let (tab_insertion, overlay_rect) = {
            // A reverse lerp of the pointers position relative to the hovered leaf rect.
            // Range is (-0.5, -0.5) to (0.5, 0.5)
            let a_pos = (Pos2::new(
                inverse_lerp(hover_rect.x_range().into(), pointer.x).unwrap(),
                inverse_lerp(hover_rect.y_range().into(), pointer.y).unwrap(),
            ) - Pos2::new(0.5, 0.5))
            .to_pos2();

            let center_drop_rect = Rect::from_center_size(
                Pos2::ZERO,
                Vec2::splat(style.overlay.feel.center_drop_coverage),
            );
            let window_drop_rect = Rect::from_center_size(
                Pos2::ZERO,
                Vec2::splat(style.overlay.feel.window_drop_coverage),
            );

            // Find out what kind of tab insertion (if any) should be used to move this widget.
            if center_drop_rect.contains(a_pos) {
                (Some(TabInsert::Append), Rect::EVERYTHING)
            } else if window_drop_rect.contains(a_pos) {
                match windows_allowed {
                    true => (None, Rect::NOTHING),
                    false => (Some(TabInsert::Append), Rect::EVERYTHING),
                }
            } else {
                // Assessing if were above/below the two linear functions x-y=0 and -x-y=0 determines
                // what "diagonal" quadrant were in.
                let a_pos = match allowed_splits {
                    AllowedSplits::All => a_pos,
                    AllowedSplits::LeftRightOnly => Pos2::new(a_pos.x, 0.0),
                    AllowedSplits::TopBottomOnly => Pos2::new(0.0, a_pos.y),
                    AllowedSplits::None => Pos2::ZERO,
                };
                if a_pos == Pos2::ZERO {
                    match windows_allowed {
                        true => (None, Rect::NOTHING),
                        false => (Some(TabInsert::Append), Rect::EVERYTHING),
                    }
                } else {
                    match (a_pos.x - a_pos.y > 0., -a_pos.x - a_pos.y > 0.) {
                        (true, true) => (
                            Some(TabInsert::Split(Split::Above)),
                            Rect::everything_above(center.y),
                        ),
                        (false, true) => (
                            Some(TabInsert::Split(Split::Left)),
                            Rect::everything_left_of(center.x),
                        ),
                        (true, false) => (
                            Some(TabInsert::Split(Split::Right)),
                            Rect::everything_right_of(center.x),
                        ),
                        (false, false) => (
                            Some(TabInsert::Split(Split::Below)),
                            Rect::everything_below(center.y),
                        ),
                    }
                }
            }
        };

        let default_value = windows_allowed.then(|| {
            TabDestination::Window(Rect::from_min_size(pointer, self.source_rect.size()).into())
        });
        let final_result = tab_insertion.map_or(default_value, |tab| match hover.dst {
            TreeComponent::Surface(surface) => Some(TabDestination::EmptySurface(surface)),
            TreeComponent::Node(path) => Some(TabDestination::Node(path, tab)),
            _ => None,
        });

        self.update_lock(hover, LockState::SoftLock, style, ui.ctx());

        // Draw the overlay
        match final_result {
            Some(TabDestination::Window(rect)) => {
                let rect = self.window_preview_rect(source, rect.into());
                let rect_bounded = constrain_rect_to_area(ui, rect, window_bounds);
                draw_window_rect(rect_bounded, ui, overlay, style);
            }
            Some(_) => {
                draw_drop_rect(hover_rect.intersect(overlay_rect), ui, overlay, style);
            }
            None => (),
        }

        final_result
    }

    fn update_lock(
        &mut self,
        hover: &HoverData,
        target_state: LockState,
        style: &Style,
        ctx: &Context,
    ) {
        match self.locked.as_mut() {
            Some(lock_time) => {
                if target_state == LockState::HardLock {
                    *lock_time = ctx.input(|i| i.time);
                }
                let window_hold = if !hover.dst.surface_address().is_main() {
                    ctx.request_repaint();
                    self.is_locked(style, ctx)
                } else {
                    false
                };
                if target_state == LockState::Unlocked && !window_hold {
                    self.locked = None;
                }
            }
            None => {
                if target_state != LockState::Unlocked {
                    self.locked = Some(ctx.input(|i| i.time));
                }
            }
        }
    }

    pub(super) fn is_locked(&self, style: &Style, ctx: &Context) -> bool {
        match self.locked.as_ref() {
            Some(lock_time) => {
                let elapsed = ctx.input(|i| (i.time - lock_time) as f32);
                ctx.request_repaint();
                elapsed < style.overlay.feel.max_preference_time
            }
            None => false,
        }
    }

    /// Takes the drag's source rather than reading it off `self`: the address of what is being
    /// carried lives in one place now (`State::carried_tab`), and this is the only thing on the
    /// destination side that still needs to know anything about it — a tab pulled out of the
    /// main surface previews smaller than one already floating in a window of its own.
    fn window_preview_rect(&self, source: DragSource, rect: Rect) -> Rect {
        if source.surface == SurfaceIndex::main() {
            Rect::from_min_size(rect.min, rect.size() * 0.8)
        } else {
            rect
        }
    }
}

#[inline(always)]
const fn lerp_vec(split: Split, alpha: f32) -> Vec2 {
    if split.is_top_bottom() {
        vec2(alpha, 0.5)
    } else {
        vec2(0.5, alpha)
    }
}

// Draws a filled rect describing where a tab will be dropped.
#[inline(always)]
fn draw_drop_rect(rect: Rect, ui: &Ui, overlay: LayerId, style: &Style) {
    let painter = make_overlay_painter(ui, overlay);
    painter.rect_filled(rect, 0.0, style.overlay.selection_color);
}

// Draws a stroked rect describing where a tab will be dropped.
#[inline(always)]
fn draw_window_rect(rect: Rect, ui: &Ui, overlay: LayerId, style: &Style) {
    let painter = make_overlay_painter(ui, overlay);
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(
            style.overlay.selection_stroke_width,
            style.overlay.selection_color,
        ),
        StrokeKind::Inside,
    );
}

/// An adapted version of the [`egui::Area`]s code for restricting an area rect to a bound.
fn constrain_rect_to_area(ui: &Ui, rect: Rect, mut bounds: Rect) -> Rect {
    if rect.width() > bounds.width() {
        // Allow overlapping side bars.
        let screen_rect = ui.ctx().content_rect();
        (bounds.min.x, bounds.max.x) = (screen_rect.min.x, screen_rect.max.x);
    }
    if rect.height() > bounds.height() {
        // Allow overlapping top/bottom bars:
        let screen_rect = ui.ctx().content_rect();
        (bounds.min.y, bounds.max.y) = (screen_rect.min.y, screen_rect.max.y);
    }

    let mut pos = rect.min;

    // Constrain to screen, unless window is too large to fit:
    let margin_x = (rect.width() - bounds.width()).at_least(0.0);
    let margin_y = (rect.height() - bounds.height()).at_least(0.0);

    pos.x = pos.x.at_most(bounds.right() + margin_x - rect.width()); // move left if needed
    pos.x = pos.x.at_least(bounds.left() - margin_x); // move right if needed
    pos.y = pos.y.at_most(bounds.bottom() + margin_y - rect.height()); // move right if needed
    pos.y = pos.y.at_least(bounds.top() - margin_y); // move down if needed

    pos = pos.round_to_pixels(ui.painter().pixels_per_point());

    Rect::from_min_size(pos, rect.size())
}
