use egui::{Context, Id, Pos2};

use super::drag_and_drop::{DragData, DragDropState, HoverData};
use crate::{Style, SurfaceIndex};

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
    /// `(junction handle id, whether the drag has moved anything yet)`, kept for as long as one
    /// of the handles at a separator crossing is held.
    ///
    /// The same question `separator_drag_start` answers — did this gesture change the layout,
    /// or was it a grab and a release — asked of a gesture that moves *two or three* fractions
    /// at once. Remembering their starting values would mean remembering a list whose length
    /// depends on the junction; what the commit event needs is only whether any of them ever
    /// moved, and each frame of the drag already answers that for itself.
    pub junction_drag: Option<(Id, bool)>,
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
