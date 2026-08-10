use egui::{Context, Id, Pos2};

use super::drag_and_drop::{DragData, DragDropState, HoverData};
use crate::{NodePath, Style, SurfaceIndex};

/// The junction a drag has hold of, remembered whole for as long as the button is down.
///
/// Not bookkeeping for its own sake. The handles are read off the geometry afresh every frame,
/// and the geometry is exactly what the drag is moving: a junction can change its index along
/// its line, change kind, or stop existing altogether while the hand is still down. So "the
/// junction at the index that reported a drag this frame" names a *different* junction from one
/// frame to the next, and the neighbours it names get moved by a gesture that never grabbed
/// them. What the gesture has hold of is decided once, at `drag_started`, and every frame after
/// that moves *those* nodes — whatever the detector now says is at that spot.
#[derive(Clone, Copy, Debug)]
pub(super) struct JunctionDrag {
    /// The handle whose press started this. Every other handle stands down while it is live —
    /// neither drawn nor interacted — so a drag cannot pick up a neighbour it passes over.
    pub id: Id,

    /// The split on whose line between its two children the junction sits. One boundary the
    /// drag moves, along that split's own axis.
    pub outer: NodePath,

    /// Orientation of `outer` itself: `true` if [`crate::Node::Horizontal`]. Which component of
    /// the drag goes to `outer` and which to `divider` is read off it.
    pub outer_horizontal: bool,

    /// The divider that ends on that line — the tee's stem. The other boundary the drag moves,
    /// across `outer`'s axis.
    pub divider: NodePath,

    /// Whether any of it has actually moved yet.
    ///
    /// The same question [`State::separator_drag_start`] answers for a single divider, asked of a
    /// gesture that moves two fractions at once. Remembering their starting values would mean
    /// remembering a pair whose meaning depends on the junction; what the commit event needs is
    /// only whether either of them ever moved, and each frame of the drag already answers that
    /// for itself.
    pub moved: bool,

    /// The pass this drag was last seen alive in — the frame the owning handle last reported it.
    ///
    /// A drag whose junction stopped existing never gets a `drag_stopped` to clear it, and an
    /// entry left behind would hold every other handle down for good. A pass number cannot go
    /// stale that way: it either names the frame before this one, or it does not.
    pub pass: u64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct State {
    pub drag_start: Option<Pos2>,
    pub last_hover_pos: Option<Pos2>,
    pub dnd: Option<DragDropState>,
    pub window_fade: Option<(f64, SurfaceIndex)>,
    /// `(separator id, its `fraction` at `drag_started()`)`, kept until
    /// `drag_stopped()`. Lets us tell a real move from "grabbed and released with
    /// no effective motion" (a click while the split is already clamped to its
    /// min/max, so the accumulated delta is zero) and skip `LayoutCommitted` in
    /// the latter case.
    ///
    /// Semantic note: these are conceptually two levels — (1) *interaction*, "the
    /// user touched the separator" (always fires on release), and (2) *state
    /// change*, "the layout actually changed" (only when
    /// `fraction_end != fraction_start`). They are currently merged into a single
    /// `LayoutCommitted`, and this guard deliberately drops level (1) in favour of
    /// (2): consumers that diff a layout snapshot get a commit event with nothing
    /// to diff otherwise. If level (1) is ever needed (telemetry, focus-on-grab),
    /// split it into a separate event rather than removing the guard.
    pub separator_drag_start: Option<(Id, f32)>,
    /// The junction handle that is being dragged, and the nodes it grabbed. See [`JunctionDrag`].
    pub junction_drag: Option<JunctionDrag>,
}

impl State {
    #[inline(always)]
    pub(super) fn load(ctx: &Context, id: Id) -> Self {
        ctx.data_mut(|d| d.get_temp(id)).unwrap_or(Self {
            drag_start: None,
            last_hover_pos: None,
            dnd: None,
            window_fade: None,
            separator_drag_start: None,
            junction_drag: None,
        })
    }

    #[inline(always)]
    pub(super) fn store(self, ctx: &Context, id: Id) {
        ctx.data_mut(|d| d.insert_temp(id, self));
    }

    pub(super) fn reset_drag(&mut self) {
        self.dnd = None;
        self.window_fade = None;
        self.drag_start = None;
    }

    pub(super) fn set_drag_and_drop(
        &mut self,
        drag: DragData,
        drop: HoverData,
        ctx: &Context,
        style: &Style,
    ) {
        if !self.is_drag_drop_locked(ctx, style) {
            self.dnd = Some(DragDropState {
                hover: Some(drop),
                drag,
                pointer: ctx.pointer_hover_pos().unwrap_or(Pos2::ZERO),
                locked: None,
            })
        }
    }

    #[inline(always)]
    fn is_drag_drop_locked(&self, ctx: &Context, style: &Style) -> bool {
        self.dnd
            .as_ref()
            .is_some_and(|drag_drop_state| drag_drop_state.is_locked(style, ctx))
    }
}
