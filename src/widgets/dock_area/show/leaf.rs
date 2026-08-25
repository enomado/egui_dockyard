use std::ops::RangeInclusive;

use egui::{
    Align, Align2, Button, Color32, CornerRadius, CursorIcon, Frame, Id, Key, LayerId, Layout,
    NumExt, Order, Popup, PopupCloseBehavior, Rect, Response, ScrollArea, Sense, Shape, Stroke,
    StrokeKind, TextStyle, Ui, UiBuilder, Vec2, WidgetText, emath::TSTransform, epaint::TextShape,
    pos2, vec2,
};

use crate::NodePath;
use crate::dock_area::ids::tab_widget_id;
use crate::dock_area::tab_removal::{ForcedRemoval, TabRemoval};
use crate::tab_viewer::OnCloseResponse;
use crate::{
    DockArea, Style, SurfaceIndex, TabAddAlign, TabIndex, TabStyle, TabViewer,
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

            // Desired size for tabs in "expanded" mode.
            let prefered_width = style
                .tab_bar
                .fill_tab_bar
                .then_some(available_width / (leaf.len() as f32));

            let tab_hovered = self.tabs(
                tabs_ui,
                state,
                path,
                tab_viewer,
                tabbar_outer_rect,
                prefered_width,
                fade_style,
            );

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
                self.tab_collapse(ui, path, tabbar_outer_rect, fade_style, collapsed)
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

    #[allow(clippy::too_many_arguments)]
    fn tabs(
        &mut self,
        tabs_ui: &mut Ui,
        state: &mut State,
        path: NodePath,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        tabbar_outer_rect: Rect,
        preferred_width: Option<f32>,
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
                            preferred_width,
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
                    preferred_width,
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
                        ui.label(tab_viewer.title(tab));
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
            Self::draw_close_window_symbol(ui, stroke_color, close_all_rect);
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

    /// Draws the collapse button.
    fn tab_collapse(
        &mut self,
        ui: &mut Ui,
        path: NodePath,
        tabbar_outer_rect: Rect,
        fade_style: Option<&Style>,
        collapsed: bool,
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
            Self::draw_chevron_down(ui, style, color, arrow_rect);
        } else {
            // Draw arrow.
            Self::draw_arrow(collapsed, ui, color, arrow_rect);
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
            } else {
                // Queued, not applied: the leaf whose collapsed flag this flips is the one
                // being drawn, and its body is still ahead in this pass. See `DockMutation`
                // for the one-frame shift this buys and the shape that would remove it.
                self.mutations.push(DockMutation::SetLeafCollapsed {
                    path,
                    collapsed: !collapsed,
                });
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

    fn draw_close_window_symbol(ui: &mut Ui, stroke_color: Color32, close_all_rect: Rect) {
        ui.painter().add(Shape::line(
            vec![
                close_all_rect
                    .right_center()
                    .lerp(close_all_rect.right_bottom(), 0.5),
                close_all_rect.right_bottom(),
                close_all_rect.left_bottom(),
                close_all_rect.left_top(),
                close_all_rect
                    .center_top()
                    .lerp(close_all_rect.left_top(), 0.5),
            ],
            Stroke::new(1.0_f32, stroke_color),
        ));
        ui.painter().line_segment(
            [close_all_rect.center_top(), close_all_rect.right_center()],
            Stroke::new(1.0_f32, stroke_color),
        );
        ui.painter().line_segment(
            [close_all_rect.center(), close_all_rect.right_top()],
            Stroke::new(1.0_f32, stroke_color),
        );
    }

    fn draw_arrow(collapsed: bool, ui: &mut Ui, color: Color32, arrow_rect: Rect) {
        ui.painter().add(Shape::convex_polygon(
            if collapsed {
                // Arrow pointing rightwards.
                vec![
                    arrow_rect.left_top(),
                    arrow_rect.right_center(),
                    arrow_rect.left_bottom(),
                ]
            } else {
                // Arrow pointing downwards.
                vec![
                    arrow_rect.left_top(),
                    arrow_rect.right_top(),
                    arrow_rect.center_bottom(),
                ]
            },
            color,
            Stroke::NONE,
        ));
    }

    fn draw_chevron_down(ui: &mut Ui, style: &Style, color: Color32, arrow_rect: Rect) {
        ui.painter().add(Shape::convex_polygon(
            // Arrow pointing downwards.
            vec![
                arrow_rect.left_top(),
                arrow_rect.right_top(),
                arrow_rect.center(),
            ],
            color,
            Stroke::NONE,
        ));

        // Chevron pointing downwards.
        ui.painter().add(Shape::convex_polygon(
            vec![
                arrow_rect.left_center(),
                arrow_rect.right_center(),
                arrow_rect.center_bottom(),
            ],
            color,
            Stroke::NONE,
        ));
        let color = style.buttons.minimize_window_bg_fill;
        ui.painter().add(Shape::convex_polygon(
            vec![
                arrow_rect
                    .left_center()
                    .lerp(arrow_rect.right_center(), 0.25),
                arrow_rect
                    .left_center()
                    .lerp(arrow_rect.right_center(), 0.75),
                arrow_rect.center().lerp(arrow_rect.center_bottom(), 0.5),
            ],
            color,
            Stroke::NONE,
        ));
    }

    /// Updates the collapsed state of the node and its parents.
    ///
    /// Called from the render epilogue, right after the collapsed flag it reads has been
    /// written, not from the click handler that requested the change.
    pub(super) fn window_update_collapsed(&mut self, path: NodePath) {
        let surface = &mut self.dock_state[path.surface];
        let collapsed = surface[path.node].is_collapsed();
        if !collapsed {
            if let Some(window_state) = self.dock_state.get_window_state_mut(path.surface) {
                window_state.set_new(true);
            }
        } else if surface.root_node().is_some_and(|root| root.is_collapsed()) {
            // Height of the window before collapsing, so expanding restores it. A root
            // that was never laid out has no height to remember.
            let surface_height = surface
                .root()
                .and_then(|root| self.layout.rect(NodePath::new(path.surface, root)))
                .map_or(0.0, |rect| rect.height());
            if let Some(window_state) = self.dock_state.get_window_state_mut(path.surface) {
                window_state.set_expanded_height(surface_height);
            }
        }
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
        label: WidgetText,
        focused: bool,
        active: bool,
        is_being_dragged: bool,
        preferred_width: Option<f32>,
        show_close_button: bool,
        fade: Option<&Style>,
    ) -> (Response, Option<Response>) {
        let style = fade.unwrap_or_else(|| self.style.as_ref().unwrap());
        let galley = label.into_galley(ui, None, f32::INFINITY, TextStyle::Button);
        let x_spacing = 8.0;
        let text_width = galley.size().x + 2.0 * x_spacing;
        let close_button_size = if show_close_button {
            Style::TAB_CLOSE_BUTTON_SIZE.min(style.tab_bar.height)
        } else {
            0.0
        };

        // Compute total width of the tab bar.
        let minimum_width = tab_style
            .minimum_width
            .unwrap_or(0.0)
            .at_least(text_width + close_button_size);
        let tab_width = preferred_width.unwrap_or(0.0).at_least(minimum_width);

        let (_, tab_rect) = ui.allocate_space(vec2(tab_width, ui.available_height()));
        let mut response = ui.interact(tab_rect, id, Sense::click_and_drag());
        if ui.ctx().dragged_id().is_none() && self.draggable_tabs {
            response = response.on_hover_cursor(CursorIcon::Grab);
        }

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
        let text_pos = {
            let pos = Align2::CENTER_CENTER.pos_in_rect(&text_rect.shrink2(vec2(x_spacing, 0.0)));
            pos - galley.size() / 2.0
        };

        ui.painter()
            .add(TextShape::new(text_pos, galley, tab_style.text_color));

        let close_response = show_close_button.then(|| {
            let mut close_button_rect = tab_rect;
            close_button_rect.set_left(text_rect.right());
            close_button_rect =
                Rect::from_center_size(close_button_rect.center(), Vec2::splat(close_button_size));

            let close_response = ui
                .interact(close_button_rect, id.with("close-button"), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);

            let color = if close_response.hovered() || close_response.has_focus() {
                style.buttons.close_tab_active_color
            } else {
                style.buttons.close_tab_color
            };

            if close_response.hovered() || close_response.has_focus() {
                let mut corner_radius = tab_style.corner_radius;
                corner_radius.nw = 0;
                corner_radius.sw = 0;

                ui.painter().rect_filled(
                    close_button_rect,
                    corner_radius,
                    style.buttons.close_tab_bg_fill,
                );
            }

            let mut x_rect = close_button_rect;
            rect_set_size_centered(&mut x_rect, Vec2::splat(Style::TAB_CLOSE_X_SIZE));
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
        if overflow > 1.0 {
            if tabbar_response.hovered() || tab_hovered {
                scroll += ui.input(|i| i.smooth_scroll_delta.y + i.smooth_scroll_delta.x);
            }
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
