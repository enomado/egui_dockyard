use std::ops::RangeInclusive;

use egui::{
    Align, AtomLayout, Atoms, Button, CornerRadius, CursorIcon, Frame, Id, Key, LayerId, Layout,
    Modifiers, NumExt, Order, Popup, PopupCloseBehavior, Rect, Response, ScrollArea, Sense, Stroke,
    StrokeKind, TextStyle, Ui, UiBuilder, Vec2, emath::TSTransform, pos2, vec2,
};

use super::glyph::{self, Dir};
use super::title::{
    FADE_LENGTH, SizedTitle, fade_out, measure_title, paint_title, strip_length, strip_slot,
};
use crate::NodePath;
use crate::core::fit::{
    MIN_SQUEEZED_TEXT, MIN_SQUEEZED_TEXT_ACTIVE, TabBarFit, TabRoom, TabWant, fit_strip_names,
    fit_tab_widths,
};
use crate::dock_area::ids::tab_widget_id;
use crate::dock_area::tab_removal::{ForcedRemoval, TabRemoval};
use crate::layout::SideStrip;
use crate::tab_viewer::OnCloseResponse;
use crate::{
    DockArea, Fold, Style, SurfaceIndex, TabAddAlign, TabIndex, TabStyle, TabViewer,
    dock_area::{
        DockMutation,
        drag_and_drop::{DragSource, HoverData, TreeComponent},
        state::{DragSubject, State},
    },
    utils::{clip_to, fade_visuals, rect_set_size_centered, rect_stroke_box},
};

fn tab_body_id(dock_area_id: Id, path: NodePath, tab_id: Id) -> Id {
    dock_area_id.with((path.surface, "surface")).with(tab_id)
}

/// Breathing room at each end of a tab's name, and how far its text sits from the tab's edge.
const TAB_TEXT_PADDING: f32 = 8.0;

/// What a strip draws in place of the names it had no room for at all.
///
/// A strip *drops* names it cannot fit, and a dropped name leaves nothing behind to fade — so
/// unlike a cut name, this one needs a mark of its own.
const OVERFLOW_MARK: &str = "…";

/// One tab as a strip names it: what to draw, and what a click on it asks for.
///
/// Collected in full before any of it is drawn, because the titles and the per-tab style come
/// from the consumer's `TabViewer` — which borrows it mutably — while drawing needs the
/// `DockArea` mutably in turn.
struct StripName {
    /// The leaf this tab lives in — the one a click brings back to the front.
    leaf: NodePath,
    tab: TabIndex,
    /// Widget address, so hover and focus stay with this name across frames.
    id: Id,
    /// Whether this is the tab its leaf is showing, which the strip keeps legible while the
    /// panel is away.
    active: bool,
    style: TabStyle,
    /// Measured once, when the list is collected: the room every name wants has to be known
    /// before any of them is placed, and the same measurement is what gets drawn afterwards.
    title: SizedTitle,
}

/// The strip a naming pass is for: which node it stands for, which way it runs, the room the
/// arrow left it, and what a click on a name asks the dock for.
///
/// The four travel together and only together — a strip is named for one node, in one place, on
/// behalf of one expansion — and they were four of `strip_names`'s eight arguments, which is a
/// group that had not been named yet.
struct StripNaming {
    /// The collapsed leaf, or the row stowed as a unit, that the strip stands for.
    path: NodePath,
    /// `Some` for a side strip, which runs down the screen; `None` for a collapsed tab bar,
    /// which runs across it. Everything the naming does is written in terms of *along* and
    /// *across*, and this is which is which.
    side: Option<SideStrip>,
    /// What is left of the strip once the arrow has taken its square.
    rect: Rect,
    /// What the arrow queues — "come back as you were". A click on a name asks for that *and*
    /// for the tab it names, so the panel returns showing what was clicked.
    expand: DockMutation,
}

impl<Tab> DockArea<'_, Tab> {
    pub(super) fn show_leaf(
        &mut self,
        ui: &mut Ui,
        state: &mut State,
        path: NodePath,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        fade_style: Option<(&Style, f32)>,
    ) {
        assert!(self.dock_state[path].is_leaf());
        let collapsed = self.dock_state[path].is_collapsed();

        // The layout pass has just assigned this leaf its rectangle; a leaf with no entry
        // was never laid out, which means there is nothing to draw.
        let Some(rect) = self.layout.rect(path) else {
            return;
        };

        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }

        let ui = &mut ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down_justified(Align::Min))
                .id_salt((path.node, "node")),
        );
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        clip_to(ui, rect);

        if self.dock_state[path].tabs_count() == 0 {
            return;
        }
        // A leaf the layout pass squeezed sideways draws a strip and no tab bar: a tab bar is
        // a row of names scrolled along x, and there is no x to scroll along here. Asked, not
        // guessed from how narrow the rectangle is — see `NodeGeometry::side_strip`.
        if let Some(side) = self.layout.side_strip(path) {
            self.collapsed_bar(
                ui,
                path,
                tab_viewer,
                fade_style.map(|(style, _)| style),
                Some(side),
                DockMutation::SetLeafFold {
                    path,
                    fold: Fold::Open,
                },
            );
        } else {
            let tabbar_rect = self.tab_bar(
                ui,
                state,
                path,
                tab_viewer,
                fade_style.map(|(style, _)| style),
                collapsed,
            );
            self.tab_body(
                ui,
                state,
                path,
                tab_viewer,
                spacing,
                tabbar_rect,
                fade_style,
                collapsed,
            );
        }

        let leaf = self.dock_state[path]
            .get_leaf()
            .expect("This node must be a leaf here");
        let forced: Vec<TabIndex> = leaf
            .iter_tabs_indexed()
            .filter_map(|(tab_index, tab)| tab_viewer.force_close(tab).then_some(tab_index))
            .collect();
        for tab_index in forced {
            self.mutations.push(DockMutation::Remove(TabRemoval::Tab(
                (path, tab_index).into(),
                ForcedRemoval(true),
            )));
        }
    }

    fn tab_bar(
        &mut self,
        ui: &mut Ui,
        state: &mut State,
        path: NodePath,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        fade_style: Option<&Style>,
        collapsed: bool,
    ) -> Rect {
        assert!(self.dock_state[path].is_leaf());

        let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
        let (tabbar_outer_rect, tabbar_response) = ui.allocate_exact_size(
            vec2(ui.available_width(), style.tab_bar.height),
            Sense::hover(),
        );
        ui.painter().rect_filled(
            tabbar_outer_rect,
            style.tab_bar.corner_radius,
            style.tab_bar.bg_fill,
        );

        let tabbar_outer_rect = tabbar_outer_rect - style.tab_bar.inner_margin;

        let mut available_width = tabbar_outer_rect.width();
        if available_width == 0.0 {
            return tabbar_outer_rect;
        }

        // Reserve space for the buttons at the ends of the tab bar.

        if self.show_add_buttons {
            available_width -= Style::TAB_ADD_BUTTON_SIZE;
        }

        if self.show_leaf_close_all_buttons {
            available_width -= Style::TAB_CLOSE_ALL_BUTTON_SIZE;
        }

        if self.show_leaf_collapse_buttons {
            available_width -= Style::TAB_COLLAPSE_BUTTON_SIZE;
        }

        // How the width is shared out among the tabs, worked out before any of them is drawn.
        // Nothing is reserved for the overflow fade: it is painted over the bar's own last few
        // pixels, so the tabs keep every one of them.
        let fit = self.tab_widths(ui, path, tab_viewer, fade_style, available_width);

        let (actual_width, tab_hovered) = {
            let leaf = self
                .dock_state
                .leaf(path)
                .expect("This node must be a leaf");

            let tabbar_inner_rect = Rect::from_min_size(
                (tabbar_outer_rect.min - pos2(-leaf.scroll, 0.0)
                    + vec2(
                        if self.show_leaf_collapse_buttons {
                            Style::TAB_COLLAPSE_BUTTON_SIZE
                        } else {
                            0.0
                        },
                        0.0,
                    ))
                .to_pos2(),
                vec2(tabbar_outer_rect.width(), tabbar_outer_rect.height()),
            );

            let tabs_ui = &mut ui.new_child(
                UiBuilder::new()
                    .max_rect(tabbar_inner_rect)
                    .layout(Layout::left_to_right(Align::Center))
                    .id_salt("tabs"),
            );

            let mut clip_rect = tabbar_outer_rect;
            clip_rect.set_width(available_width);
            if self.show_leaf_collapse_buttons {
                clip_rect = clip_rect.translate(vec2(Style::TAB_COLLAPSE_BUTTON_SIZE, 0.0));
            }
            // Narrowed through `clip_to`, never assigned: the tab bar is always a full
            // `tab_bar.height` tall while the leaf it sits in may be squeezed shorter than
            // that, and an assignment here hands that difference a licence to paint through
            // the enclosing window's border and out onto the desktop. A leaf with no room for
            // its tab bar must show a *cut* tab bar.
            clip_to(tabs_ui, clip_rect);

            let tab_hovered = self.tabs(
                tabs_ui,
                state,
                path,
                tab_viewer,
                tabbar_outer_rect,
                &fit,
                fade_style,
            );

            // The tabs that did not fit are past the right-hand edge, so that edge is where the
            // bar says they exist: the last of it fades into the bar's own background, the way
            // Firefox fades beside its scroll arrows. Painted outside `tabs_ui`, and so outside
            // the scroll — what it states ("there is more here") holds wherever the bar has been
            // scrolled to.
            if fit.overflow {
                let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
                let fade = Rect::from_min_max(
                    pos2(clip_rect.right() - FADE_LENGTH, tabbar_outer_rect.top()),
                    pos2(clip_rect.right(), tabbar_outer_rect.bottom()),
                );
                fade_out(ui, fade, style.tab_bar.bg_fill, false);
            }

            // Draw hline from tab end to edge of tab bar.
            let px = ui.ctx().pixels_per_point().recip();
            let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());

            ui.painter().hline(
                tabs_ui.min_rect().right().min(clip_rect.right())..=tabbar_outer_rect.right(),
                tabbar_outer_rect.bottom() - px,
                (px, style.tab_bar.hline_color),
            );

            // Add button at the ends of the tab bar.
            if self.show_add_buttons {
                let offset = match style.buttons.add_tab_align {
                    TabAddAlign::Left => {
                        (clip_rect.width() - tabs_ui.min_rect().width()).at_least(0.0)
                    }
                    TabAddAlign::Right => 0.0,
                } + if self.show_leaf_close_all_buttons {
                    Style::TAB_CLOSE_ALL_BUTTON_SIZE
                } else {
                    0.0
                };
                self.tab_plus(ui, path, tab_viewer, tabbar_outer_rect, offset, fade_style);
            }

            if self.show_leaf_close_all_buttons {
                // Current leaf contains non-closable tabs.
                let disabled = self
                    .dock_state
                    .leaf(path)
                    .map(|leaf| !leaf.iter_tabs().all(|tab| tab_viewer.is_closeable(tab)))
                    .expect("This node must be a leaf");

                // Current window contains non-closable tabs.
                let close_window_disabled = disabled
                    || !self.dock_state[path.surface].iter().all(|node| {
                        node.get_leaf().is_none_or(|leaf| {
                            leaf.iter_tabs().all(|tab| tab_viewer.is_closeable(tab))
                        })
                    });

                self.tab_close_all(
                    ui,
                    path,
                    tabbar_outer_rect,
                    fade_style,
                    disabled,
                    close_window_disabled,
                )
            }

            if self.show_leaf_collapse_buttons {
                self.tab_collapse(
                    ui,
                    path,
                    tabbar_outer_rect,
                    fade_style,
                    collapsed,
                    None,
                    DockMutation::SetLeafFold {
                        path,
                        // A plain click spends **height**: the leaf becomes a tab bar and keeps
                        // its column, which is what folding meant before an axis could be
                        // chosen. Spending width is the same arrow with Ctrl held — see
                        // `strip_target`.
                        fold: if collapsed { Fold::Open } else { Fold::Bar },
                    },
                )
            }

            (tabs_ui.min_rect().width(), tab_hovered)
        };

        self.tab_bar_scroll(
            ui,
            path,
            actual_width,
            available_width,
            &tabbar_response,
            tab_hovered,
        );

        tabbar_outer_rect
    }

    /// How wide each tab in this leaf's bar gets to be, given the room the bar has.
    ///
    /// Asked before anything is drawn, because how the width is shared out is a question about
    /// the whole bar: a tab cannot be given its share out of whatever is left when the bar
    /// reaches it, which is what "wide as its name, and the rest scrolls off" amounted to.
    ///
    /// What a tab *wants* is its name laid out in full, plus padding, plus its close button, and
    /// never less than the style's own `minimum_width`. What it can be squeezed to is that same
    /// furniture around [`MIN_SQUEEZED_TEXT`] — or its own width, if it is already narrower.
    fn tab_widths(
        &self,
        ui: &Ui,
        path: NodePath,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        fade: Option<&Style>,
        available_width: f32,
    ) -> TabBarFit {
        let style = fade.unwrap_or_else(|| self.style.as_ref().unwrap());
        let leaf = self
            .dock_state
            .leaf(path)
            .expect("This node must be a leaf");

        let mut wants: Vec<TabWant> = Vec::with_capacity(leaf.len());
        let mut fixed = 0.0;
        for index in 0..leaf.len() {
            let tab_index = TabIndex(index);
            let tab = &leaf[tab_index];
            let active = leaf.is_active(tab_index);
            let least = if active {
                MIN_SQUEEZED_TEXT_ACTIVE
            } else {
                MIN_SQUEEZED_TEXT
            };
            let tab_style = tab_viewer
                .tab_style_override(tab, &style.tab)
                .unwrap_or_else(|| style.tab.clone());

            let close = if self.show_close_buttons && tab_viewer.is_closeable(tab) {
                style.buttons.close_tab_size.min(style.tab_bar.height)
            } else {
                0.0
            };
            let title = measure_title(ui, tab_viewer.title(tab)).length(false);

            // `minimum_width` is a minimum for the whole tab, furniture included, so what it asks
            // of the *name* is that much less the button beside it.
            let name = (title + 2.0 * TAB_TEXT_PADDING)
                .at_least(tab_style.minimum_width.unwrap_or(0.0) - close);
            wants.push(TabWant {
                name,
                furniture: close,
                // Under pressure the active tab keeps its close button: it is the one you are
                // reading, and the one a crowded bar is closed from.
                keeps_furniture: active,
                // A floor, not a width: a tab whose name is shorter than the minimum keeps its own
                // width rather than being padded out to a minimum it does not need. The button an
                // active tab keeps sits on top of that, since it is drawn beside the name and not
                // over it.
                floor: name.min(2.0 * TAB_TEXT_PADDING + least) + if active { close } else { 0.0 },
            });
            if index != 0 {
                fixed += tab_style.spacing;
            }
        }

        // `fill_tab_bar` is the same question asked from the other side: it says a tab may be
        // *widened* to an equal share when there is room going spare. Expressed as a want rather
        // than as a second rule, so that a bar which is both filled and overfull still squeezes.
        if style.tab_bar.fill_tab_bar && !wants.is_empty() {
            let equal = (available_width - fixed) / wants.len() as f32;
            for want in &mut wants {
                want.name = want.name.at_least(equal - want.furniture);
            }
        }

        fit_tab_widths(&wants, fixed, available_width)
    }

    #[allow(clippy::too_many_arguments)]
    fn tabs(
        &mut self,
        tabs_ui: &mut Ui,
        state: &mut State,
        path: NodePath,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        tabbar_outer_rect: Rect,
        fit: &TabBarFit,
        fade: Option<&Style>,
    ) -> bool {
        let mut tab_hovered = false;

        assert!(self.dock_state[path].is_leaf());

        let focused = self.dock_state.focused_leaf();
        let tabs_len = self.dock_state[path]
            .get_leaf()
            .expect("This node must be a leaf here")
            .len();

        for tab_index in 0..tabs_len {
            let tab_index = TabIndex(tab_index);
            // The widget address is the tab's identity, so what egui hangs off it — focus,
            // hover, a drag in flight — stays with this tab when the bar is edited around it.
            let tab_id = self
                .dock_state
                .leaf(path)
                .unwrap()
                .tab_id_at(tab_index)
                .expect("the loop runs over the positions this leaf has");
            let id = tab_widget_id(self.id, path, tab_id);
            let is_being_dragged = tabs_ui.ctx().is_being_dragged(id)
                && tabs_ui.input(|i| i.pointer.is_decidedly_dragging())
                && self.draggable_tabs;

            if is_being_dragged {
                tabs_ui.output_mut(|o| o.cursor_icon = CursorIcon::Grabbing);

                // The hand has closed on this tab: name it in the one place that remembers what
                // is being dragged, the same way a separator and a junction do at their own
                // `drag_started`. This *is* the tab gesture's `drag_started`: egui reports the
                // drag on the tab's own widget id, and the first frame it does is the first frame
                // this branch runs — there is no `Response` here to ask, because the dragged tab
                // is drawn into a layer of its own and interacted with under a second id.
                //
                // Told from the frames after it by the field itself rather than by a flag: the
                // gesture is either already there under this id, or it is not.
                let subject = DragSubject::Tab(DragSource {
                    surface: path.surface,
                    node: path.node,
                    tab: tab_id,
                });
                let pass = tabs_ui.ctx().cumulative_pass_nr();
                if state.in_flight().is_some_and(|drag| drag.widget == id) {
                    state.keep_drag_alive(id, pass);
                } else if let Some(started_at) = tabs_ui.ctx().pointer_interact_pos() {
                    // Named on the first dragged frame that has a pointer position to name it
                    // *from*, which is what `drag_start`'s `get_or_insert` amounted to: a gesture
                    // with no origin has no delta to measure, so there is nothing for the frame
                    // below to do either way. Both halves therefore appear together or not at all.
                    state.begin_drag(id, subject, started_at, pass);
                }
            }

            let (is_active, label, tab_style, closeable) = {
                let leaf = self.dock_state[path]
                    .get_leaf()
                    .expect("This node must be a leaf");
                let style = fade.unwrap_or_else(|| self.style.as_ref().unwrap());
                let tab_style = tab_viewer.tab_style_override(&leaf[tab_index], &style.tab);
                (
                    leaf.is_active(tab_index) || is_being_dragged,
                    tab_viewer.title(&leaf[tab_index]),
                    tab_style.unwrap_or(style.tab.clone()),
                    tab_viewer.is_closeable(&leaf[tab_index]),
                )
            };

            let show_close_button = self.show_close_buttons && closeable;

            let (response, title_id) = if is_being_dragged {
                let layer_id = LayerId::new(Order::Tooltip, id);
                let response = tabs_ui
                    .scope_builder(UiBuilder::new().layer_id(layer_id), |ui| {
                        self.tab_title(
                            ui,
                            &tab_style,
                            id,
                            label,
                            is_active && Some(path) == focused,
                            is_active,
                            is_being_dragged,
                            fit.rooms[tab_index.0],
                            fit.crowded,
                            show_close_button,
                            fade,
                        )
                    })
                    .response;
                let title_id = response.id;

                let response =
                    tabs_ui.interact(response.rect, id.with("dragged"), Sense::click_and_drag());

                if let Some(pointer_pos) = tabs_ui.ctx().pointer_interact_pos() {
                    // The gesture was named above, off this same position — the one branch that
                    // does not name it is the one this `if let` also skips.
                    let start = state
                        .in_flight()
                        .expect("the tab in the hand was named on the frame it was grabbed")
                        .started_at;
                    let delta = pointer_pos - start;
                    if delta.x.abs() > 30.0 || delta.y.abs() > 6.0 {
                        // Past the pull-out threshold: the tab now follows the pointer and a drop
                        // becomes possible. Recorded in the gesture rather than re-derived by
                        // whoever needs to know — see `DragInFlight::moved`. This is the *one*
                        // expression of "the tab has been pulled out"; a second copy of it in the
                        // shape of "there is a rect in memory this frame" is what it used to be.
                        state.mark_drag_moved();
                        tabs_ui
                            .ctx()
                            .transform_layer_shapes(layer_id, TSTransform::new(delta, 1.0));
                    }
                }

                (response, title_id)
            } else {
                if tab_index.0 != 0 {
                    tabs_ui.allocate_space(vec2(tab_style.spacing, 0.0));
                }
                let (mut response, close_response) = self.tab_title(
                    tabs_ui,
                    &tab_style,
                    id,
                    label,
                    is_active && Some(path) == focused,
                    is_active,
                    is_being_dragged,
                    fit.rooms[tab_index.0],
                    fit.crowded,
                    show_close_button,
                    fade,
                );
                let title_id = response.id;
                let close_clicked = close_response.is_some_and(|res| res.clicked());
                let is_lonely_tab = self.dock_state[path.surface].num_tabs() == 1;

                if self.show_tab_name_on_hover {
                    let tab = self.dock_state[path]
                        .get_leaf()
                        .expect("This node must be a leaf")
                        .tab_at(tab_index)
                        .expect("this tab was just drawn");
                    response = response.on_hover_ui(|ui| {
                        // The same title the tab shows, icon included, held to one line high so
                        // that an icon of any resolution stays an icon here too.
                        let line = ui.text_style_height(&TextStyle::Body);
                        ui.add(AtomLayout::new(tab_viewer.title(tab)).max_height(line));
                    });
                }

                if self.tab_context_menus {
                    let eject_button =
                        Button::new(&self.dock_state.translations.tab_context_menu.eject_button);
                    let close_button =
                        Button::new(&self.dock_state.translations.tab_context_menu.close_button);

                    response.context_menu(|ui| {
                        let leaf = self.dock_state[path]
                            .get_leaf()
                            .expect("This node must be a leaf");
                        let already_active = leaf.is_active(tab_index);
                        let tab = &leaf[tab_index];

                        tab_viewer.context_menu(ui, tab, path);
                        if (path.surface.is_main() || !is_lonely_tab)
                            && tab_viewer.allowed_in_windows(tab)
                            && ui.add(eject_button).clicked()
                        {
                            self.mutations
                                .push(DockMutation::Detach((path, tab_index).into()));
                            ui.close();
                        }
                        if show_close_button && ui.add(close_button).clicked() {
                            match tab_viewer.on_close(tab) {
                                OnCloseResponse::Close => {
                                    self.mutations.push(DockMutation::Remove(TabRemoval::Tab(
                                        (path, tab_index).into(),
                                        ForcedRemoval(false),
                                    )))
                                }
                                OnCloseResponse::Focus => {
                                    // Only count as a finalised event if `active` actually
                                    // changes; both the epilogue's activation and its focus
                                    // push are guarded the same way, so a no-op
                                    // close-on-already-active-tab emits nothing.
                                    if !already_active {
                                        self.mutations
                                            .push(DockMutation::Activate((path, tab_index).into()));
                                    }
                                    self.mutations.push(DockMutation::Focus(path));
                                }
                                OnCloseResponse::Ignore => (),
                            }
                            ui.close();
                        }
                    });
                }

                if close_clicked {
                    self.mutations.push(DockMutation::Remove(TabRemoval::Tab(
                        (path, tab_index).into(),
                        ForcedRemoval(false),
                    )));
                }

                if let Some(pos) = state.last_hover_pos {
                    // Use response.rect.contains instead of
                    // response.hovered as the dragged tab covers
                    // the underlying tab
                    //
                    // No `carried_tab().is_some()` guard here: an idle hover writing this rect is
                    // inert, because the only reader (`show/mod.rs`) gates on `carried` itself
                    // before it ever looks at `tab_hover_rect` — see
                    // `tests/hovering_with_nothing_carried_does_nothing.rs`.
                    if response.rect.contains(pos) {
                        self.tab_hover_rect = Some((response.rect, tab_index));
                    }
                }

                (response, title_id)
            };

            if response.hovered() {
                tab_hovered = true;
            }

            // Paint hline below each tab unless its active (or option says otherwise).
            let leaf = self.dock_state.leaf(path).unwrap();
            let already_active = leaf.is_active(tab_index);
            let tab = &leaf[tab_index];
            let style = fade.unwrap_or_else(|| self.style.as_ref().unwrap());
            let tab_style = tab_viewer.tab_style_override(tab, &style.tab);
            let tab_style = tab_style.as_ref().unwrap_or(&style.tab);

            if !is_active || tab_style.hline_below_active_tab_name {
                let px = tabs_ui.ctx().pixels_per_point().recip();
                tabs_ui.painter().hline(
                    response.rect.x_range(),
                    tabbar_outer_rect.bottom() - px,
                    (px, style.tab_bar.hline_color),
                );
            }

            if response.clicked()
                || (tabs_ui.memory(|m| m.has_focus(title_id))
                    && tabs_ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space)))
            {
                // Queued rather than applied: the leaf's body is drawn later in this same
                // pass, so the click frame still paints the previous tab and the new one
                // appears on the next repaint. `DockMutation` documents that shift and the
                // shape that would remove it.
                if !already_active {
                    self.mutations
                        .push(DockMutation::Activate((path, tab_index).into()));
                }
                self.mutations.push(DockMutation::Focus(path));
            }

            tab_viewer.on_tab_button(tab, &response);

            if self.show_close_buttons && tab_viewer.is_closeable(tab) && response.middle_clicked()
            {
                self.mutations.push(DockMutation::Remove(TabRemoval::Tab(
                    (path, tab_index).into(),
                    ForcedRemoval(false),
                )));
            }
        }

        tab_hovered
    }

    /// Draws the tab add button.
    #[allow(clippy::too_many_arguments)]
    fn tab_plus(
        &mut self,
        ui: &mut Ui,
        path: NodePath,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        tabbar_outer_rect: Rect,
        offset: f32,
        fade_style: Option<&Style>,
    ) {
        let rect = Rect::from_min_max(
            tabbar_outer_rect.right_top() - vec2(Style::TAB_ADD_BUTTON_SIZE + offset, 0.0),
            tabbar_outer_rect.right_bottom() - vec2(offset, 2.0),
        );

        let ui = &mut ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center))
                .id_salt((path.node, "tab_add")),
        );

        let (rect, mut response) = ui.allocate_exact_size(ui.available_size(), Sense::click());

        response = response.on_hover_cursor(CursorIcon::PointingHand);

        let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
        let color = if response.hovered() || response.has_focus() {
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, style.buttons.add_tab_bg_fill);
            style.buttons.add_tab_active_color
        } else {
            style.buttons.add_tab_color
        };

        let mut plus_rect = rect;

        rect_set_size_centered(&mut plus_rect, Vec2::splat(Style::TAB_ADD_PLUS_SIZE));

        ui.painter().line_segment(
            [plus_rect.center_top(), plus_rect.center_bottom()],
            Stroke::new(1.0_f32, color),
        );
        ui.painter().line_segment(
            [plus_rect.right_center(), plus_rect.left_center()],
            Stroke::new(1.0_f32, color),
        );

        // Draw button left border.
        ui.painter().vline(
            rect.left(),
            rect.y_range(),
            Stroke::new(
                ui.ctx().pixels_per_point().recip(),
                style.buttons.add_tab_border_color,
            ),
        );

        let popup_id = ui.id().with("tab_add_popup");
        if self.show_add_popup {
            Popup::from_toggle_button_response(&response)
                .id(popup_id)
                .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| {
                    tab_viewer.add_popup(ui, path);
                });
        }

        if response.clicked() {
            tab_viewer.on_add(path);
        }
    }

    /// Draws the close all button.
    #[allow(clippy::too_many_arguments)]
    #[allow(unused_assignments)]
    fn tab_close_all(
        &mut self,
        ui: &mut Ui,
        path: NodePath,
        tabbar_outer_rect: Rect,
        fade_style: Option<&Style>,
        disabled: bool,
        close_window_disabled: bool,
    ) {
        let rect = Rect::from_min_max(
            tabbar_outer_rect.right_top() - vec2(Style::TAB_CLOSE_ALL_BUTTON_SIZE, 0.0),
            tabbar_outer_rect.right_bottom() - vec2(0.0, 2.0),
        );

        let ui = &mut ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center))
                .id_salt((path.node, "tab_close_all")),
        );

        let (rect, mut response) = ui.allocate_exact_size(ui.available_size(), Sense::click());

        let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());

        // Whether we're on "secondary button mode" due to modifier keys
        let on_secondary_button = self.is_on_secondary_button(path.surface, ui, &response);

        let mut stroke_color = if disabled {
            style.buttons.close_all_tabs_disabled_color
        } else if response.hovered() || response.has_focus() {
            if !(close_window_disabled && on_secondary_button) {
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::ZERO,
                    style.buttons.close_all_tabs_bg_fill,
                );
            }
            style.buttons.close_all_tabs_active_color
        } else {
            style.buttons.close_all_tabs_color
        };

        let mut close_all_rect = rect;

        rect_set_size_centered(&mut close_all_rect, Vec2::splat(Style::TAB_CLOSE_ALL_SIZE));

        if !disabled {
            response = response.on_hover_cursor(CursorIcon::PointingHand);
        }

        if on_secondary_button {
            // Close the entire window
            if close_window_disabled {
                stroke_color = style.buttons.close_all_tabs_disabled_color;
                response = response
                    .on_hover_cursor(CursorIcon::NotAllowed)
                    .on_hover_text(
                        self.dock_state
                            .translations
                            .leaf
                            .close_all_button_disabled_tooltip
                            .as_str(),
                    );
            }
            glyph::close_window(ui.painter(), close_all_rect, stroke_color);
        } else {
            // Close all tabs in this leaf
            if !disabled {
                // "Close the window" is offered only where there is a window to close, and
                // `as_window` is what says so — the main surface simply has no `WindowIndex`
                // to put in the request.
                if let Some(window) = path.surface.as_window()
                    && self.secondary_button_context_menu
                {
                    response.context_menu(|ui| {
                        ui.add_enabled_ui(!close_window_disabled, |ui| {
                            if ui
                                .button(&self.dock_state.translations.leaf.close_all_button)
                                .on_disabled_hover_text(
                                    self.dock_state
                                        .translations
                                        .leaf
                                        .close_all_button_disabled_tooltip
                                        .as_str(),
                                )
                                .clicked()
                            {
                                self.mutations
                                    .push(DockMutation::Remove(TabRemoval::Window(window)));
                            }
                        });
                    });
                }
            } else {
                response = response
                    .on_hover_cursor(CursorIcon::NotAllowed)
                    .on_hover_text(
                        self.dock_state
                            .translations
                            .leaf
                            .close_button_disabled_tooltip
                            .as_str(),
                    );
            }

            if response.clicked() {
                if on_secondary_button {
                    // `on_secondary_button` is false on the main surface, so this always
                    // names a window; asking for it by type keeps that from being a comment.
                    if let Some(window) = path.surface.as_window()
                        && !close_window_disabled
                    {
                        self.mutations
                            .push(DockMutation::Remove(TabRemoval::Window(window)));
                    }
                } else if !disabled {
                    self.mutations
                        .push(DockMutation::Remove(TabRemoval::Node(path)));
                }
            }

            ui.painter().line_segment(
                [close_all_rect.left_top(), close_all_rect.right_bottom()],
                Stroke::new(1.0_f32, stroke_color),
            );
            ui.painter().line_segment(
                [close_all_rect.right_top(), close_all_rect.left_bottom()],
                Stroke::new(1.0_f32, stroke_color),
            );
        }

        // Draw button left border.
        ui.painter().vline(
            rect.left(),
            rect.y_range(),
            Stroke::new(
                ui.ctx().pixels_per_point().recip(),
                style.buttons.close_all_tabs_border_color,
            ),
        );

        if !disabled && !on_secondary_button {
            response = self.show_tooltip_hints(path.surface, response);
        }
    }

    /// Draws something that is collapsed down to one arrow and nothing else: a leaf squeezed
    /// sideways into a strip, or a whole side stowed away.
    ///
    /// One function for both, because they draw the same thing. What differs is what the arrow
    /// *means*, and that arrives as `on_toggle` rather than being worked out from the node —
    /// the two edits address different kinds of node (`set_leaf_collapsed` panics on a split)
    /// and only whoever put the arrow here knows which one this is.
    ///
    /// `side` is [`None`] when the parent is a vertical split: the collapsed thing spends height
    /// there, so what it gets is an ordinary horizontal bar and the arrow points the usual way.
    ///
    /// The arrow is drawn whatever [`DockArea::show_leaf_collapse_buttons`] says. That knob is
    /// about the button on a *tab bar*, where hiding it leaves the tabs themselves to click
    /// on; a bar like this has nothing else in it, so hiding the arrow there would leave no way
    /// back except in code.
    fn collapsed_bar(
        &mut self,
        ui: &mut Ui,
        path: NodePath,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        fade_style: Option<&Style>,
        side: Option<SideStrip>,
        on_toggle: DockMutation,
    ) {
        let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
        let bar_height = style.tab_bar.height;
        let corner_radius = style.tab_bar.corner_radius;
        let bg_fill = style.tab_bar.bg_fill;

        let (strip_rect, _response) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        ui.painter().rect_filled(strip_rect, corner_radius, bg_fill);

        // `tab_collapse` cuts its button out of the top-left of what it is given, one
        // `TAB_COLLAPSE_BUTTON_SIZE` wide and as tall as the rectangle — so handing it the top
        // `tab_bar.height` of the strip gives exactly the same square as on a tab bar. A
        // horizontal bar is already exactly that tall, so the same expression covers it.
        let button_rect = Rect::from_min_size(strip_rect.min, vec2(strip_rect.width(), bar_height));
        self.tab_collapse(
            ui,
            path,
            button_rect,
            fade_style,
            true,
            side,
            on_toggle.clone(),
        );

        // Everything the arrow did not take says *what* is put away here. Which end of the
        // rectangle that is follows from the same fact as the button's own shape: a strip is a
        // column with the square at its top, a bar is a row with the square at its left.
        let names_rect = if side.is_some() {
            Rect::from_min_max(
                pos2(strip_rect.left(), strip_rect.top() + bar_height),
                strip_rect.max,
            )
        } else {
            Rect::from_min_max(
                pos2(
                    strip_rect.left() + Style::TAB_COLLAPSE_BUTTON_SIZE,
                    strip_rect.top(),
                ),
                strip_rect.max,
            )
        };
        self.strip_names(
            ui,
            tab_viewer,
            fade_style,
            StripNaming {
                path,
                side,
                rect: names_rect,
                expand: on_toggle,
            },
        );
    }

    /// Names the tabs that a strip stands for, along whatever the arrow left of it.
    ///
    /// A panel put away should not hide *which* panels went with it: the strip is already proof
    /// that something is there, and a blank one is proof of nothing else. For a collapsed leaf
    /// these are its own tabs; for a side stowed as a unit they are every leaf inside it, in tree
    /// order, with a hairline between leaves so that three panels do not read as one long list.
    ///
    /// `expand` is what the arrow queues — "come back as you were". A click on a name asks for
    /// that *and* for the tab it names, so the panel returns showing what was clicked rather than
    /// whatever happened to be active when it went away.
    ///
    /// A strip is as long as the panel it stands beside and never longer, so the names have to
    /// live within whatever that is: they are squeezed and truncated first, and what is still
    /// left over is stood for by a single ellipsis at the end. See [`fit_strip_names`].
    fn strip_names(
        &mut self,
        ui: &mut Ui,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        fade_style: Option<&Style>,
        naming: StripNaming,
    ) {
        let StripNaming {
            path,
            side,
            rect,
            expand,
        } = naming;
        // A strip runs down the screen and a bar runs across it. Everything below is written in
        // terms of *along* and *across* so that the two share one set of arithmetic, and the axis
        // is read from the layout's answer rather than guessed from which side is longer.
        let vertical = side.is_some();
        let (start, end) = if vertical {
            (rect.top(), rect.bottom())
        } else {
            (rect.left(), rect.right())
        };
        // Gathered before anything is drawn: the titles come from the consumer, which wants
        // `&mut tab_viewer` while the tree is borrowed, and painting wants `&mut self` back.
        let names = self.strip_name_list(ui, tab_viewer, path, fade_style);
        if names.is_empty() {
            return;
        }

        let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
        let hairline = style.separator.color_idle;
        // The ellipsis is not a tab and cannot be clicked, so it is drawn in the plainest of the
        // three tab colours rather than in one that would offer something it does not have.
        let overflow_color = style.tab.inactive.text_color;
        // The mark sits on the strip itself rather than on a name's background, and it is short
        // enough never to be cut — this is what it would fade into if it ever were.
        let style_bg_fill = style.tab_bar.bg_fill;
        let line = ui.ctx().pixels_per_point().recip();

        // What stands in front of each name: a hairline before the first name of every leaf but
        // the first, so it lands between two groups rather than at the top of the strip.
        let gaps: Vec<f32> = names
            .iter()
            .enumerate()
            .map(|(index, name)| match index.checked_sub(1) {
                Some(previous) if names[previous].leaf != name.leaf => line,
                _ => 0.0,
            })
            .collect();
        // What every name would like, asked before any of it is placed: how the room is shared
        // out is a question about the whole list, and a name cannot be given its share out of
        // what happens to be left when the list reaches it.
        let naturals: Vec<f32> = names
            .iter()
            .map(|name| strip_length(&name.title, vertical))
            .collect();
        let overflow = measure_title(ui, Atoms::new(OVERFLOW_MARK));
        let fit = fit_strip_names(
            &naturals,
            &gaps,
            end - start,
            strip_length(&overflow, vertical),
        );

        let mut cursor = start;
        for (index, name) in names.into_iter().take(fit.lengths.len()).enumerate() {
            if gaps[index] > 0.0 {
                let separator = strip_slot(rect, vertical, cursor, gaps[index]);
                ui.painter().rect_filled(separator, 0.0, hairline);
                cursor += gaps[index];
            }

            // The name is laid out whole and shown for as far as it was given — the slot is the
            // shorter of the two, and what runs past it is clipped and faded rather than cut.
            let length = strip_length(&name.title, vertical).min(fit.lengths[index]);
            let name_rect = strip_slot(rect, vertical, cursor, length);
            cursor += length;

            let response = ui
                .interact(name_rect, name.id, Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);

            // The same three states a tab bar shows, drawn from the same style: which panel was
            // open stays legible while it is away, and hover feedback is whatever the user
            // already configured rather than a second palette invented here.
            let tab_style = if name.active {
                &name.style.active
            } else if response.hovered() || response.has_focus() {
                &name.style.hovered
            } else {
                &name.style.inactive
            };
            ui.painter()
                .rect_filled(name_rect, tab_style.corner_radius, tab_style.bg_fill);

            paint_title(
                ui,
                name_rect,
                name.title,
                tab_style.text_color,
                tab_style.bg_fill,
                vertical,
                Some(&response),
            );

            if response.clicked() {
                // Two mutations, in this order, and both already exist: the panel comes back and
                // the tab that was asked for is the one showing. `Activate` is applied before the
                // expansion for no reason other than that it reads as the answer to the click.
                self.mutations
                    .push(DockMutation::Activate((name.leaf, name.tab).into()));
                self.mutations.push(expand.clone());
            }
        }

        if fit.overflow {
            // One ellipsis for everything the strip could not hold, drawn along the strip like a
            // name so that it reads as the list carrying on. It says the one thing a strip that
            // simply stopped could not: that there is more here than is written. It is a mark and
            // not a name — there is no single tab behind it for a click to bring back.
            let slot = strip_slot(rect, vertical, cursor, strip_length(&overflow, vertical));
            paint_title(
                ui,
                slot,
                overflow,
                overflow_color,
                style_bg_fill,
                vertical,
                None,
            );
        }
    }

    /// What a strip at `path` has to name, in the order it will come back in.
    ///
    /// One leaf's tabs, or — for a side stowed as a unit — every leaf inside it, depth first and
    /// first child first, which is top-to-bottom and left-to-right on screen.
    fn strip_name_list(
        &mut self,
        ui: &Ui,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        path: NodePath,
        fade_style: Option<&Style>,
    ) -> Vec<StripName> {
        let mut leaves = Vec::new();
        let mut stack = vec![path.node];
        while let Some(node) = stack.pop() {
            if self.dock_state[path.surface][node].is_leaf() {
                leaves.push(NodePath::new(path.surface, node));
            } else if let Some(children) = self.dock_state[path.surface].children(node) {
                // Pushed back to front: the stack hands the first child back first.
                stack.extend(children.iter().rev().copied());
            }
        }

        let mut names = Vec::new();
        for leaf_path in leaves {
            let count = self.dock_state[leaf_path]
                .get_leaf()
                .expect("collected as a leaf just above")
                .len();
            for tab_index in 0..count {
                let tab_index = TabIndex(tab_index);
                let leaf = self.dock_state[leaf_path]
                    .get_leaf()
                    .expect("collected as a leaf just above");
                let tab_id = leaf
                    .tab_id_at(tab_index)
                    .expect("the loop runs over the positions this leaf has");
                let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
                names.push(StripName {
                    leaf: leaf_path,
                    tab: tab_index,
                    // A salt of its own on top of the tab's address: the same tab can be named
                    // here and — once the panel is back — in its own tab bar, and two widgets
                    // sharing an id would share the hover and focus egui hangs off it.
                    id: tab_widget_id(self.id, leaf_path, tab_id).with("strip"),
                    active: leaf.is_active(tab_index),
                    style: tab_viewer
                        .tab_style_override(&leaf[tab_index], &style.tab)
                        .unwrap_or_else(|| style.tab.clone()),
                    title: measure_title(ui, tab_viewer.title(&leaf[tab_index])),
                });
            }
        }
        names
    }

    /// Draws a side that was stowed: the bar above, standing for a whole subtree.
    ///
    /// A stowed split never goes through [`Self::show_leaf`] — it is not a leaf, it has no tabs
    /// to put in a bar, and this frame its subtree is not on the geometry map at all. So the
    /// entry point is its own, and it lives here rather than with the splits because what it
    /// draws *is* the leaf's strip; only the meaning of the click differs.
    ///
    /// One arrow for the whole side, and under it the names of every tab inside — see
    /// [`Self::strip_names`], which is why this needs the consumer's `TabViewer` at all: the
    /// titles it draws are the consumer's to give.
    pub(super) fn show_stowed_split(
        &mut self,
        ui: &mut Ui,
        path: NodePath,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        fade_style: Option<&Style>,
    ) {
        debug_assert!(self.dock_state[path].is_stowed());

        // No rectangle means not on screen — a side stowed inside another stowed side. The same
        // early return as `show_leaf`, and it is the same statement: drawing shows what the
        // layout pass laid out.
        let Some(rect) = self.layout.rect(path) else {
            return;
        };
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return;
        }
        let side = self.layout.side_strip(path);

        let ui = &mut ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down_justified(Align::Min))
                .id_salt((path.node, "node")),
        );
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        clip_to(ui, rect);

        self.collapsed_bar(
            ui,
            path,
            tab_viewer,
            fade_style,
            side,
            DockMutation::SetSplitStowed {
                path,
                stowed: false,
            },
        );
    }

    /// Draws the collapse button.
    ///
    /// `on_toggle` is what a primary click asks for. Handed in rather than built here, because
    /// the same button appears on a leaf's tab bar, on a leaf squeezed into a strip and on a
    /// whole side stowed away, and those are three different edits to three different things.
    #[allow(clippy::too_many_arguments)]
    fn tab_collapse(
        &mut self,
        ui: &mut Ui,
        path: NodePath,
        tabbar_outer_rect: Rect,
        fade_style: Option<&Style>,
        collapsed: bool,
        side_strip: Option<SideStrip>,
        on_toggle: DockMutation,
    ) {
        let rect = Rect::from_min_max(
            tabbar_outer_rect.left_top(),
            tabbar_outer_rect.left_bottom() + vec2(Style::TAB_COLLAPSE_BUTTON_SIZE, 0.0),
        );

        let ui = &mut ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center))
                .id_salt((path.node, "tab_collapse")),
        );

        let (rect, mut response) = ui.allocate_exact_size(ui.available_size(), Sense::click());

        response = response.on_hover_cursor(CursorIcon::PointingHand);

        let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());

        // Whether we're on "secondary button mode" due to modifier keys
        let on_secondary_button = self.is_on_secondary_button(path.surface, ui, &response);
        // …and, where that mode does not apply, whether the same modifier turns this arrow into
        // "put my whole side away". The window action wins where both could fire: it is the
        // older meaning of the modifier, and it only exists on a floating surface at all.
        let stow_target = if on_secondary_button {
            None
        } else {
            self.stow_target(path, ui, &response)
        };
        // The third meaning, on the other key: fold this leaf the *other* way. Shift asks about
        // a bigger target (the whole side), Ctrl about a different axis (width instead of
        // height) — two questions, two keys, and neither can be mistaken for the other because
        // both are read as an exact match. See `docs/MODIFIERS.md`.
        let strip_target = self.strip_target(path, ui, &response);

        let color = if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(
                rect,
                CornerRadius::ZERO,
                style.buttons.collapse_tabs_bg_fill,
            );
            style.buttons.collapse_tabs_active_color
        } else {
            style.buttons.collapse_tabs_color
        };

        let mut arrow_rect = rect;
        rect_set_size_centered(&mut arrow_rect, Vec2::splat(Style::TAB_COLLAPSE_ARROW_SIZE));

        if on_secondary_button {
            // Collapse the entire window
            glyph::chevron(
                ui.painter(),
                arrow_rect,
                Dir::Down,
                color,
                style.buttons.minimize_window_bg_fill,
            );
        } else if stow_target.is_some() {
            glyph::stow_arrow(ui.painter(), arrow_rect, color);
        } else if let Some((_, towards)) = strip_target {
            // Where the leaf is about to go, or come back from — the same question the plain
            // arrow answers below, asked of the other axis.
            glyph::triangle(ui.painter(), arrow_rect, towards, color);
        } else {
            // The arrow points at the space this leaf will expand into: away from the edge a
            // sideways strip is pressed against, rightwards out of a collapsed tab bar, and down
            // at the body an open one heads. A side strip's answer wins where both could apply,
            // which is why it is asked first.
            let towards = match (side_strip, collapsed) {
                (Some(SideStrip::Left), _) => Dir::Right,
                (Some(SideStrip::Right), _) => Dir::Left,
                (None, true) => Dir::Right,
                (None, false) => Dir::Down,
            };
            glyph::triangle(ui.painter(), arrow_rect, towards, color);
        }

        // Draw button right border.
        ui.painter().vline(
            rect.right(),
            rect.y_range(),
            Stroke::new(
                ui.ctx().pixels_per_point().recip(),
                style.buttons.collapse_tabs_border_color,
            ),
        );

        if response.clicked() {
            if on_secondary_button {
                self.window_request_toggle_minimized(path.surface);
            } else if let Some(target) = stow_target {
                self.mutations.push(DockMutation::SetSplitStowed {
                    path: target,
                    stowed: true,
                });
            } else if let Some((fold, _)) = strip_target {
                self.mutations.push(DockMutation::SetLeafFold { path, fold });
            } else {
                // Queued, not applied: what this flips belongs to the node being drawn, and the
                // rest of that node is still ahead in this pass. See `DockMutation` for the
                // one-frame shift this buys and the shape that would remove it.
                self.mutations.push(on_toggle);
            }
        }

        if !path.surface.is_main() && self.secondary_button_context_menu {
            response.context_menu(|ui| {
                if ui
                    .button(&self.dock_state.translations.leaf.minimize_button)
                    .clicked()
                {
                    ui.close();
                    self.window_request_toggle_minimized(path.surface);
                }
            });
        }

        if !on_secondary_button {
            self.show_tooltip_hints(path.surface, response);
        }
    }

    fn show_tooltip_hints(&mut self, surface_index: SurfaceIndex, response: Response) -> Response {
        if !surface_index.is_main()
            && self.show_secondary_button_hint
            && (self.secondary_button_context_menu || self.secondary_button_on_modifier)
        {
            let hint = if self.secondary_button_context_menu && self.secondary_button_on_modifier {
                &self
                    .dock_state
                    .translations
                    .leaf
                    .minimize_button_modifier_menu_hint
            } else if self.secondary_button_context_menu {
                &self.dock_state.translations.leaf.minimize_button_menu_hint
            } else {
                &self
                    .dock_state
                    .translations
                    .leaf
                    .minimize_button_modifier_hint
            };
            return response.on_hover_text(hint);
        }
        response
    }

    fn is_on_secondary_button(
        &self,
        surface_index: SurfaceIndex,
        ui: &mut Ui,
        response: &Response,
    ) -> bool {
        !surface_index.is_main()
            && self.secondary_button_on_modifier
            && ui.input(|i| {
                i.modifiers
                    .matches_logically(self.secondary_button_modifiers)
            })
            && (response.hovered() || response.has_focus() || response.is_pointer_button_down_on())
    }

    /// The split this collapse arrow would put away while the modifier is held, or [`None`] if
    /// the gesture does not apply here.
    ///
    /// The target is **the whole side** the arrow sits in — [`Tree::top_level_ancestor`], the
    /// child of the root this node belongs to — and not merely its parent. The gesture is "put
    /// this side away", so it has to mean the same thing from any leaf in it, however deep: a
    /// side of three leaves is two splits, and a parent-sized target would take two clicks to
    /// clear and a third for four leaves. Which panel of the side was clicked is not part of
    /// what the user asked (decision of 2026-08-28, Стас: "shift-click on any of them moves the
    /// whole split").
    ///
    /// The same modifier as the secondary button, deliberately: one meaning, "the bigger version
    /// of this action", and a user who rebinds it rebinds both. Hovering is required for the same
    /// reason it is there — otherwise every arrow in the dock would change its icon the moment
    /// the key went down.
    ///
    /// [`None`] where the gesture would add nothing. A leaf that is *itself* a side is put away
    /// sideways by [`strip_target`](Self::strip_target), one key over — the same picture for one
    /// panel, and nothing for this gesture to add; and a side already stowed is what the plain
    /// arrow brings back.
    ///
    /// Behind [`DockArea::collapse_sideways`] because the layout is: with the knob off a side
    /// stowed under a horizontal split would draw one bar and leave the rest of its column to
    /// nobody, which is exactly the hole that knob exists to avoid.
    fn stow_target(&self, path: NodePath, ui: &mut Ui, response: &Response) -> Option<NodePath> {
        if !self.collapse_sideways
            || !ui.input(|i| {
                i.modifiers
                    .matches_logically(self.secondary_button_modifiers)
            })
            || !(response.hovered() || response.has_focus() || response.is_pointer_button_down_on())
        {
            return None;
        }
        let tree = &self.dock_state[path.surface];
        // `None` for the root, which is no side but the division itself.
        let side = tree.top_level_ancestor(path.node)?;
        (!tree[side].is_leaf() && !tree[side].is_stowed())
            .then(|| NodePath::new(path.surface, side))
    }

    /// What this collapse arrow would do to *this leaf's axis* while `Ctrl` is held — the fold to
    /// ask for, and the direction to draw the arrow — or [`None`] where the gesture does not
    /// apply.
    ///
    /// # Why a second modifier rather than a second button
    ///
    /// Folding spends one of the leaf's two dimensions, and which one is a real choice: a bar
    /// keeps the column and empties it, a strip hands the column to the sibling. The dock used to
    /// take that choice off the parent split — horizontal parent, strip; vertical parent, bar —
    /// which reads as a rule until the day it is the wrong one, and then there is no gesture to
    /// say so. [`Fold`] is where the choice lives now, and this is where a hand makes it.
    ///
    /// The key is `Ctrl` and not the [`secondary_button_modifiers`](DockArea::secondary_button_modifiers)
    /// this crate's other arrow gestures read, because it answers a different question. `Shift`
    /// means *a bigger target* — this leaf's whole side ([`stow_target`](Self::stow_target)).
    /// `Ctrl` means *the other axis*, on the same target. Two questions, two keys, and both read
    /// as an exact match, so a hand resting on one is never taken for the other.
    ///
    /// # When it is [`None`]
    ///
    /// * the knob is off — [`DockArea::collapse_sideways`] is what admits strips at all;
    /// * the arrow is not the one under the pointer, or `Ctrl` is not the exact chord held;
    /// * this is not a leaf: the same arrow is drawn for a *stowed side*, which has no axis of
    ///   its own to choose (its column is already what it gave up);
    /// * the parent is not a horizontal row. Width given up under a vertical parent has nobody
    ///   to take it, which is the hole `collapse_sideways` was written to avoid — so there is
    ///   nothing here to offer, and the arrow keeps its plain meaning.
    fn strip_target(&self, path: NodePath, ui: &mut Ui, response: &Response) -> Option<(Fold, Dir)> {
        if !self.collapse_sideways
            || !ui.input(|i| i.modifiers.matches_logically(Modifiers::COMMAND))
            || !(response.hovered() || response.has_focus() || response.is_pointer_button_down_on())
        {
            return None;
        }
        let tree = &self.dock_state[path.surface];
        if !tree[path.node].is_leaf() {
            return None;
        }
        let parent = tree.parent(path.node)?;
        if !tree[parent].is_horizontal() {
            return None;
        }

        if tree[path.node].fold() == Fold::Strip {
            // Already sideways: the modifier takes it back, and the arrow points where the leaf
            // will expand into — away from the edge it is pressed against.
            let towards = match self.layout.side_strip(path) {
                Some(SideStrip::Right) => Dir::Left,
                // `None` for a strip the layout did not honour (the knob turned off under a
                // stored layout): it is drawn as a bar, and its way back out is rightwards, the
                // same as any bar's.
                Some(SideStrip::Left) | None => Dir::Right,
            };
            return Some((Fold::Open, towards));
        }

        // On its way out: towards the edge of its own row it will be pressed against. Only the
        // last child goes to the trailing edge — the same rule `cut_runs` lays the strips out by,
        // so the arrow promises what the next frame draws.
        let children = tree.children(parent).expect("a horizontal node is a row");
        let is_last = children.last() == Some(&path.node);
        Some((
            Fold::Strip,
            if is_last { Dir::Right } else { Dir::Left },
        ))
    }

    /// * `active` means "the tab that is opened in the parent panel".
    /// * `focused` means "the tab that was last interacted with".
    ///
    /// Returns the main button response plus the response of the close button, if any.
    #[allow(clippy::too_many_arguments)]
    fn tab_title(
        &mut self,
        ui: &mut Ui,
        tab_style: &TabStyle,
        id: Id,
        label: Atoms<'static>,
        focused: bool,
        active: bool,
        is_being_dragged: bool,
        room: TabRoom,
        crowded: bool,
        show_close_button: bool,
        fade: Option<&Style>,
    ) -> (Response, Option<Response>) {
        let style = fade.unwrap_or_else(|| self.style.as_ref().unwrap());
        let x_spacing = TAB_TEXT_PADDING;

        // Laid out in full, however long it is. The width was decided for the bar as a whole
        // (`tab_widths`), and what does not fit inside it is clipped and faded rather than cut:
        // the ellipsis a cut would need costs about ten pixels of a name that has thirty.
        let title = measure_title(ui, label);

        let (_, tab_rect) = ui.allocate_space(vec2(room.width, ui.available_height()));
        let mut response = ui.interact(tab_rect, id, Sense::click_and_drag());
        if ui.ctx().dragged_id().is_none() && self.draggable_tabs {
            response = response.on_hover_cursor(CursorIcon::Grab);
        }

        // A crowded bar gives up its close buttons: those pixels matter more to a name with thirty
        // than a second way to close a tab does, and the active tab — the one you are reading —
        // keeps its own. It is the *bar* that decides, so every tab lets go at the same width; see
        // [`TabBarFit::crowded`] for what it looked like when each tab decided for itself.
        let button = style.buttons.close_tab_size.min(style.tab_bar.height);
        // The width of the tab was shared out knowing which buttons would be drawn, so only a
        // button the bar paid for takes room away from the name.
        let close_button_size = if show_close_button && (!crowded || active) {
            button
        } else {
            0.0
        };

        // The pointer brings the button back on whichever tab it is over, the way Chrome does —
        // but on a crowded bar it comes back *over* the name rather than beside it. Taking the
        // room from the name instead would re-cut the title of whichever tab the pointer crossed,
        // so simply moving the mouse along the bar would set the names jumping.
        let show_close_button = show_close_button && (!crowded || active || response.hovered());
        let overlaid = show_close_button && close_button_size == 0.0;

        let tab_style = if focused || is_being_dragged {
            if response.has_focus() {
                &tab_style.focused_with_kb_focus
            } else {
                &tab_style.focused
            }
        } else if active {
            if response.has_focus() {
                &tab_style.active_with_kb_focus
            } else {
                &tab_style.active
            }
        } else if response.hovered() {
            &tab_style.hovered
        } else if response.has_focus() {
            &tab_style.inactive_with_kb_focus
        } else {
            &tab_style.inactive
        };

        // Draw the full tab first and then the stroke on top to avoid the stroke
        // mixing with the background color.
        ui.painter()
            .rect_filled(tab_rect, tab_style.corner_radius, tab_style.bg_fill);
        let stroke_rect = rect_stroke_box(tab_rect, 1.0);
        ui.painter().rect_stroke(
            stroke_rect,
            tab_style.corner_radius,
            Stroke::new(1.0_f32, tab_style.outline_color),
            StrokeKind::Inside,
        );
        if !is_being_dragged {
            // Make the tab name area connect with the tab ui area.
            ui.painter().hline(
                RangeInclusive::new(
                    stroke_rect.min.x + f32::max(tab_style.corner_radius.sw.into(), 1.5),
                    stroke_rect.max.x - f32::max(tab_style.corner_radius.se.into(), 1.5),
                ),
                stroke_rect.bottom(),
                Stroke::new(2.0_f32, tab_style.bg_fill),
            );
        }

        let mut text_rect = tab_rect;
        text_rect.set_width(text_rect.width() - close_button_size);
        let inner = text_rect.shrink2(vec2(x_spacing, 0.0));

        // A title that fits is centred, as it always was; one that does not is pinned to the left
        // and faded where it runs out of tab. Both rules live in `paint_title`, which a side
        // strip's names go through as well — the two differ in which way they run, not in how
        // they treat a title too long for its slot.
        paint_title(
            ui,
            inner,
            title,
            tab_style.text_color,
            tab_style.bg_fill,
            false,
            Some(&response),
        );

        let close_response = show_close_button.then(|| {
            // Always at the tab's own right-hand end, whether the width was reserved for it or the
            // button is standing over the name. `close_tab_y_offset` moves the whole button, hit
            // target included: a mark that sits lower than what answers the click would be a
            // button that misses on purpose.
            let mut close_button_rect = tab_rect;
            close_button_rect.set_left(tab_rect.right() - button);
            close_button_rect =
                Rect::from_center_size(close_button_rect.center(), Vec2::splat(button))
                    .translate(vec2(0.0, style.buttons.close_tab_y_offset));

            if overlaid {
                // Nothing was taken off the name for this button, so the glyphs run underneath it.
                // They are faded into the tab the same way a name is faded where it runs out of
                // tab — the button stands on the background rather than on the letters.
                fade_out(ui, close_button_rect, tab_style.bg_fill, false);
            }

            let close_response = ui
                .interact(close_button_rect, id.with("close-button"), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);

            let color = if close_response.hovered() || close_response.has_focus() {
                style.buttons.close_tab_active_color
            } else {
                style.buttons.close_tab_color
            };

            if close_response.hovered() || close_response.has_focus() {
                // A disc, not a slab: the button is a mark on the tab rather than a division of
                // it. Filling its whole 24 px square — which is the full height of the bar, with
                // the tab's own corner radius on two of its corners — reads as the right-hand end
                // of the tab lighting up, which is what Chrome's small circle avoids.
                ui.painter().circle_filled(
                    close_button_rect.center(),
                    style.buttons.close_tab_mark_radius,
                    style.buttons.close_tab_bg_fill,
                );
            }

            let mut x_rect = close_button_rect;
            rect_set_size_centered(&mut x_rect, Vec2::splat(style.buttons.close_tab_x_size));
            ui.painter().line_segment(
                [x_rect.left_top(), x_rect.right_bottom()],
                Stroke::new(1.0_f32, color),
            );
            ui.painter().line_segment(
                [x_rect.right_top(), x_rect.left_bottom()],
                Stroke::new(1.0_f32, color),
            );

            close_response
        });

        (response, close_response)
    }

    #[allow(clippy::too_many_arguments)]
    fn tab_bar_scroll(
        &mut self,
        ui: &mut Ui,
        path: NodePath,
        actual_width: f32,
        available_width: f32,
        tabbar_response: &Response,
        tab_hovered: bool,
    ) {
        if available_width <= 0.0 {
            return;
        }

        let current = self
            .dock_state
            .leaf(path)
            .expect("This node must be a leaf")
            .scroll;
        let overflow = (actual_width - available_width).at_least(0.0);

        // Compare to 1.0 and not 0.0 to avoid reacting to a sub-pixel overflow from tab
        // layout. The tab bar owns its scroll position, but deliberately has no visual
        // scroll widget: wheel input above the tabs is all of the interaction.
        let mut scroll = current;
        if overflow > 1.0 && (tabbar_response.hovered() || tab_hovered) {
            scroll += ui.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
        }

        // The clamp runs on every frame, not only on a scrolled one: the overflow it clamps
        // against changes whenever the leaf is resized or a tab comes and goes, and a
        // position left outside it would scroll the bar off its own end. Only an actual
        // change is queued, so a still tab bar asks for nothing.
        let scroll = scroll.clamp(-overflow, 0.0);
        if scroll != current {
            self.mutations
                .push(DockMutation::SetLeafScroll { path, scroll });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tab_body(
        &mut self,
        ui: &mut Ui,
        state: &State,
        path: NodePath,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        spacing: Vec2,
        tabbar_rect: Rect,
        fade: Option<(&Style, f32)>,
        collapsed: bool,
    ) {
        let (body_rect, _body_response) =
            ui.allocate_exact_size(ui.available_size_before_wrap(), Sense::hover());

        // The leaf's own rectangle (tab bar plus body) — assigned by the layout pass
        // earlier this frame, and used further below for hover hit-testing.
        let rect = self
            .layout
            .rect(path)
            .expect("the body of a leaf is only drawn after the leaf was laid out");
        // What the body rectangle was on the previous frame: `TabViewer::on_rect_changed`
        // must fire exactly when it moves, so the comparison needs the old value before
        // this frame's is recorded.
        let previous_viewport = self.layout.viewport(path);

        let leaf = self
            .dock_state
            .leaf(path)
            .expect("This node must be a leaf");
        if !collapsed && let Some(tab) = leaf.active_focused() {
            if previous_viewport != Some(body_rect) {
                self.layout.set_viewport(path, body_rect);
                tab_viewer.on_rect_changed(tab);
            }

            if ui.input(|i| i.pointer.any_click())
                && let Some(pos) = state.last_hover_pos
                && body_rect.contains(pos)
                && Some(ui.layer_id()) == ui.ctx().layer_id_at(pos)
            {
                self.mutations.push(DockMutation::Focus(path));
            }

            let (style, fade_factor) = fade.unwrap_or_else(|| (self.style.as_ref().unwrap(), 1.0));
            let tabs_styles = tab_viewer.tab_style_override(tab, &style.tab);

            let tabs_style = tabs_styles.as_ref().unwrap_or(&style.tab);

            if tab_viewer.clear_background(tab) {
                ui.painter().rect_filled(
                    body_rect,
                    tabs_style.tab_body.corner_radius,
                    tabs_style.tab_body.bg_fill,
                );
            }

            // Construct a new ui with the correct tab id.
            //
            // We are forced to use `Ui::new` because other methods (eg: push_id) always mix
            // the provided id with their own which would cause tabs to change id when moved
            // from node to node.
            let id = tab_body_id(self.id, path, tab_viewer.id(tab));
            ui.ctx().check_for_id_clash(id, body_rect, "a tab with id");
            let ui = &mut Ui::new(
                ui.ctx().clone(),
                id,
                UiBuilder::new().max_rect(body_rect).layer_id(ui.layer_id()),
            );
            clip_to(ui, Rect::from_min_max(ui.cursor().min, ui.clip_rect().max));

            // Use initial spacing for ui.
            ui.spacing_mut().item_spacing = spacing;

            // Offset the background rectangle up to hide the top border behind the clip rect.
            // To avoid anti-aliasing lines when the stroke width is not divisible by two, we
            // need to calculate the effective anti-aliased stroke width.
            let effective_stroke_width = (tabs_style.tab_body.stroke.width / 2.0).ceil() * 2.0;
            let tab_body_rect = Rect::from_min_max(
                ui.clip_rect().min - vec2(0.0, effective_stroke_width),
                ui.clip_rect().max,
            );
            ui.painter().rect_stroke(
                rect_stroke_box(tab_body_rect, tabs_style.tab_body.stroke.width),
                tabs_style.tab_body.corner_radius,
                tabs_style.tab_body.stroke,
                StrokeKind::Inside,
            );

            ScrollArea::new(tab_viewer.scroll_bars(tab)).show(ui, |ui| {
                Frame::new()
                    .inner_margin(tabs_style.tab_body.inner_margin)
                    .show(ui, |ui| {
                        if fade_factor != 1.0 {
                            fade_visuals(ui.visuals_mut(), fade_factor);
                        }
                        // NOTE: deliberately no `ui.expand_to_include_rect(available_rect)` here.
                        // At this point `available_rect` is the viewport minus the top/left
                        // `inner_margin` (the Frame already moved the cursor), so expanding to it
                        // and then letting the Frame add the bottom/right margin on top made the
                        // content exceed the viewport by 1-2px — every tab body rendered a
                        // "phantom" scroll bar with ~1px of travel.
                        // Without the expand, `min_rect` is the actual content, so the scroll bar
                        // shows up only on real overflow. `ui.available_size()` inside
                        // `tab_viewer.ui()` is unchanged, so canvases that allocate the available
                        // size still fill the body as before.
                        tab_viewer.ui(ui, &*tab);
                    });
            });
        }

        // change hover destination
        if let Some(pointer) = state.last_hover_pos {
            // Prevent borrow checker issues.
            let rect = rect.to_owned();

            // if the dragged tab isn't allowed in a window,
            // it's unnecessary to change the hover state
            let carried = state.carried_tab();
            let is_dragged_valid = match carried {
                Some(src) => match src.resolve(self.dock_state) {
                    Some(src_path) => {
                        let leaf = self.dock_state.leaf(src_path.node_path()).unwrap();
                        tab_viewer.allowed_in_windows(&leaf[src_path.tab])
                            || path.surface == SurfaceIndex::main()
                    }
                    // The dragged tab left the tree during its own drag. The pass that noticed
                    // it has already ended the drag; nothing here has an opinion left to have.
                    None => true,
                },
                None => true,
            };

            // Use rect.contains instead of response.hovered as the dragged tab covers
            // the underlying responses.
            //
            // No `carried.is_some()` guard here, for the same reason `tab_hover_rect`'s write
            // above lost one: `is_dragged_valid` is already `true` when `carried` is `None` (see
            // its match above), so this condition would degrade to `rect.contains(pointer)`
            // regardless — and the write it guards is inert without a carried tab, because
            // `show/mod.rs` never reads `hover_data` without `source_rect`, which requires
            // `carried` itself. See `tests/hovering_with_nothing_carried_does_nothing.rs`.
            if rect.contains(pointer) && is_dragged_valid {
                let on_title_bar = tabbar_rect.contains(pointer);
                let (dst, tab) = {
                    match self.tab_hover_rect {
                        Some((rect, tab_index)) => {
                            (TreeComponent::Tab((path, tab_index).into()), Some(rect))
                        }
                        None => (
                            TreeComponent::Node(path),
                            on_title_bar.then_some(tabbar_rect),
                        ),
                    }
                };

                ui.memory_mut(|mem| {
                    mem.data.insert_temp(
                        self.id.with("hover_data"),
                        Some(HoverData { rect, dst, tab }),
                    );
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tab_body_id;
    use crate::{DockState, NodePath, SurfaceIndex};
    use egui::Id;

    #[test]
    fn tab_body_ids_differ_between_surfaces() {
        let dock_area_id = Id::new("dock-area");
        let tab_id = Id::new("same-tab");
        // The same node identity seen through two surfaces must still give two ids: the
        // surface is part of the address, and ids are only unique within one tree.
        let dock_state = DockState::new(vec!["a tab"]);
        let node = dock_state.main_surface().root().unwrap();

        let main = NodePath {
            surface: SurfaceIndex::main(),
            node,
        };
        let detached = NodePath {
            surface: SurfaceIndex::window(0),
            node,
        };

        assert_ne!(
            tab_body_id(dock_area_id, main, tab_id),
            tab_body_id(dock_area_id, detached, tab_id)
        );
    }
}
