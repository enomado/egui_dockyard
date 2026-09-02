use egui::{
    Align, AtomLayout, Atoms, Color32, Context, CornerRadius, CursorIcon, Frame, Id, LayerId,
    Layout, Order, Rect, Response, RichText, Sense, Shape, Stroke, Ui, UiBuilder, Vec2, vec2,
};

use crate::{
    DockArea, NodePath, Style, SurfaceIndex, TabViewer, WindowIndex,
    dock_area::{
        DockMutation,
        events::DockEvent,
        show::{border_clearance, collapsed_strip_height},
        state::{DragSubject, State, WindowEdge},
        tab_removal::TabRemoval,
    },
    utils::{fade_visuals, rect_set_size_centered},
};

/// The side or corner of a window's frame paired with the id salt egui's own
/// `do_resize_interaction` builds that widget's id with (`window.rs`, `egui`). Order matches
/// egui's: sides first, then corners.
const WINDOW_EDGES: [(WindowEdge, &str); 8] = [
    (WindowEdge::Right, "right"),
    (WindowEdge::Left, "left"),
    (WindowEdge::Bottom, "bottom"),
    (WindowEdge::Top, "top"),
    (WindowEdge::RightBottom, "right_bottom"),
    (WindowEdge::RightTop, "right_top"),
    (WindowEdge::LeftBottom, "left_bottom"),
    (WindowEdge::LeftTop, "left_top"),
];

/// Everything between a floating window's outer height and the height the dock gets to draw in.
///
/// `Window::min_height` / `max_height` name the **outer** size of a window — egui says so on the
/// methods themselves — while every height the dock computes (a strip of collapsed tab bars, the
/// height a window is restored to when it expands) is measured in the content area inside all of
/// this. Handing one to the other unconverted is a window short by exactly this much, which is
/// how a collapsed window came to be 14 px too small at the default style: enough that its last
/// row of tabs was drawn over its own bottom border.
///
/// Three things stand between the two, and all three are here so that no caller has to remember
/// the list:
///
/// * the window frame — its margin and its stroke;
/// * [`Style::dock_area_padding`], which `allocate_area_for_root_node` takes off the top;
/// * the clearance the same function keeps from the border it draws, at both ends.
fn window_chrome_height(frame: &Frame, style: &Style) -> f32 {
    let padding = style.dock_area_padding.map_or(0.0, |margin| {
        f32::from(margin.top) + f32::from(margin.bottom)
    });
    let clearance = border_clearance(style);
    frame.total_margin().sum().y + padding + clearance.top + clearance.bottom
}

impl<Tab> DockArea<'_, Tab> {
    pub(super) fn show_window_surface(
        &mut self,
        ui: &Ui,
        window: WindowIndex,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        state: &mut State,
        fade_style: Option<(&Style, f32, SurfaceIndex)>,
    ) {
        let surf_index = SurfaceIndex::Window(window);
        // This id is not a debug string, it is *format*: egui remembers a window's position
        // and size under it, and that memory is persisted across restarts. It is therefore
        // frozen at the shape the old positional `SurfaceIndex` printed — the same numbering
        // a saved layout uses, main being 0 — so that changing how the dock addresses windows
        // internally does not scatter everyone's floating windows across the screen.
        let id = format!("window SurfaceIndex({})", window.0 + 1).into();
        let bounds = self.window_bounds.unwrap();
        let open = true;

        // Calculate fading of the window (if any)
        let (fade_factor, fade_style) = match fade_style {
            Some((style, factor, surface_index)) => {
                if surface_index == surf_index {
                    (1.0, None)
                } else {
                    (factor, Some((style, factor)))
                }
            }
            None => (1.0, None),
        };

        // Get galley of currently selected node as a window title
        let title = {
            let node_id = self.dock_state[surf_index]
                .focused_leaf()
                .unwrap_or_else(|| {
                    self.dock_state[surf_index]
                        .breadth_first()
                        .into_iter()
                        .find(|node| self.dock_state[surf_index][*node].is_leaf())
                        .expect("a window surface should never be empty")
                });
            let leaf = self.dock_state[surf_index][node_id].get_leaf().unwrap();
            let active = leaf
                .active_focused()
                .expect("a window surface should never hold an empty leaf");
            let mut title = tab_viewer.title(active);
            // The window paints its own title rather than letting the tab style speak, so the
            // colour is applied to whatever text the title carries; an icon beside it is left
            // alone, being already the colour it was given.
            let color = ui.visuals().widgets.noninteractive.fg_stroke.color;
            title.map_texts(|text| text.color(color));
            title
        };

        // Iterate through every node in dock_state[surf_index], and sum up the number of tabs in them
        let tab_count: usize = self.dock_state[surf_index]
            .iter()
            .map(|n| n.tabs_count())
            .sum();

        // Fade window frame (if necessary)
        let mut frame = Frame::window(ui.style());
        if fade_factor != 1.0 {
            frame.fill = frame.fill.linear_multiply(fade_factor);
            frame.stroke.color = frame.stroke.color.linear_multiply(fade_factor);
            frame.shadow.color = frame.shadow.color.linear_multiply(fade_factor);
        }

        // Every height below is measured in the *content* area the dock draws in; a window is
        // sized by its **outer** height. `chrome` is the whole of the difference, and it is
        // applied in exactly the three places a window's height is decided: here for a
        // minimized window, here for a collapsed one, and inside `create_window` for the
        // height an expanding window is restored to.
        let (chrome, minimized_height, collapsed_height) = {
            let style = self.style.as_ref().unwrap();
            let chrome = window_chrome_height(&frame, style);
            let rows = self.dock_state[surf_index].collapsed_leaf_count();
            (
                chrome,
                // A minimized window is one row: expand button, title, and the tab count.
                style.tab_bar.height + chrome,
                collapsed_strip_height(rows, style) + chrome,
            )
        };

        let (egui_window, took_expanded_height) = crate::dock_area::window_ui::create_window(
            self.dock_state.get_window_state(surf_index).unwrap(),
            id,
            bounds,
            chrome,
        );
        self.mutations.push(DockMutation::WindowShown {
            surface: surf_index,
            took_expanded_height,
        });

        let minimized = self
            .dock_state
            .get_window_state(surf_index)
            .unwrap()
            .is_minimized();
        let shown = if minimized {
            egui_window
                .resizable([true, false])
                .max_height(minimized_height)
                .min_height(minimized_height)
        } else if self.dock_state[surf_index].is_collapsed() {
            egui_window
                .resizable([true, false])
                .max_height(collapsed_height)
                .min_height(collapsed_height)
        } else {
            egui_window
        }
        .frame(frame)
        .show(ui.ctx(), |ui| {
            // Fade inner ui (if necessary)
            if fade_factor != 1.0 {
                fade_visuals(ui.visuals_mut(), fade_factor);
            }
            if minimized {
                self.minimized_body(
                    ui,
                    surf_index,
                    fade_style.map(|(style, _)| style),
                    title,
                    tab_count,
                )
            } else {
                self.render_nodes(ui, tab_viewer, state, surf_index, fade_style);
            }
        });

        // `None` only for a window that is closed; this one has no `open` flag to close it with.
        if let Some(inner) = shown {
            self.follow_window_move(&inner.response, surf_index, state);
            self.follow_window_resize(ui.ctx(), id, surf_index, state);
        }

        if !open {
            self.mutations
                .push(DockMutation::Remove(TabRemoval::Window(window)));
        }
    }

    /// Puts a window the hand is moving into the field that says what the hand holds.
    ///
    /// The gesture is egui's, not the dock's: a window is built with no title bar, which egui
    /// resolves to drag-from-anywhere over the window's body, and it is `Window::show`'s own
    /// returned response — the area's `"move"` widget — that reports it. Nothing here takes the
    /// drag over or moves anything; egui has already moved the window by the time this runs. What
    /// it does is *name* the gesture, so that "what is being dragged right now" has one answer
    /// for a window as it does for a tab or a boundary, and so that the id the dock reports
    /// compares equal to [`egui::Context::dragged_id`] (see [`DragInFlight::widget`]).
    ///
    /// A window move commits nothing: where a floating window sits is egui's area memory, not
    /// the dock's tree, so there is no layout change for a consumer to diff and no
    /// [`DockEvent::LayoutCommitted`] to send. `moved` is still recorded, because it is the
    /// gesture's own question — "has any of it actually moved yet" — and a consumer asking "is
    /// the layout being edited right now" asks it of every subject alike.
    ///
    /// [`DragInFlight::widget`]: crate::DragInFlight::widget
    fn follow_window_move(&self, response: &Response, surf_index: SurfaceIndex, state: &mut State) {
        let pass = response.ctx.cumulative_pass_nr();

        if response.drag_started() {
            state.begin_drag(
                response.id,
                DragSubject::Window {
                    surface: surf_index,
                    edge: None,
                },
                response
                    .interact_pointer_pos()
                    .expect("a drag that started was pressed somewhere"),
                pass,
            );
        }

        if response.dragged() {
            // Alive this frame, so a stale entry can be told from a live one — a window whose
            // surface is closed under the hand is never drawn again, and so never reports the
            // release that would end it.
            state.keep_drag_alive(response.id, pass);
            // Asked of the pointer and not of any stored geometry, for the same reason a carried
            // tab's `moved` is: the drag writes into egui's area state, which the dock does not
            // keep, so "did this gesture do anything" is a question about the hand.
            if response.drag_delta() != Vec2::ZERO
                && state
                    .in_flight()
                    .is_some_and(|drag| drag.widget == response.id)
            {
                state.mark_drag_moved();
            }
        }

        if response.drag_stopped() {
            state.end_drag(response.id);
        }
    }

    /// Puts a window edge or corner the hand is resizing into the field that says what the hand
    /// holds.
    ///
    /// Unlike a move, egui hands the dock no response for this: `Window::show`'s return value is
    /// built solely from the area's own drag-from-anywhere, and the eight resize widgets — one
    /// per side, one per corner — are created and consumed entirely inside egui's
    /// `do_resize_interaction`, which never surfaces past `show`. What is read here is not a
    /// response `show` handed the dock; it is the *same* widget, read back by the id egui itself
    /// built for it — [`Context::read_response`] against
    /// `Id::new(LayerId::new(Order::Middle, window_id)).with("edge_drag").with(<side>)`, which is
    /// exactly how `do_resize_interaction` names them (`window.rs`, `egui`; `WINDOW_EDGES` above
    /// carries the eight salts). Nothing here takes the drag over or resizes anything; egui has
    /// already resized the window by the time this runs.
    ///
    /// The id derivation is an implementation detail of egui's, not part of its public contract —
    /// the same honesty risk [`follow_window_move`](Self::follow_window_move) carries for the
    /// move gesture, and it wants the same canary: a test that fails loud if egui ever stops
    /// answering at these ids, rather than the field going quietly empty while a window resizes
    /// under the hand.
    fn follow_window_resize(
        &self,
        ctx: &Context,
        window_id: Id,
        surf_index: SurfaceIndex,
        state: &mut State,
    ) {
        let pass = ctx.cumulative_pass_nr();
        let base = Id::new(LayerId::new(Order::Middle, window_id)).with("edge_drag");

        for (edge, salt) in WINDOW_EDGES {
            // `None` for a side this window is not resizable along this frame (an axis locked by
            // `resizable([...])`, or the window simply not resizable) — egui never created the
            // widget, so there is nothing to read.
            let Some(response) = ctx.read_response(base.with(salt)) else {
                continue;
            };

            if response.drag_started() {
                state.begin_drag(
                    response.id,
                    DragSubject::Window {
                        surface: surf_index,
                        edge: Some(edge),
                    },
                    response
                        .interact_pointer_pos()
                        .expect("a drag that started was pressed somewhere"),
                    pass,
                );
            }

            if response.dragged() {
                state.keep_drag_alive(response.id, pass);
                if response.drag_delta() != Vec2::ZERO
                    && state
                        .in_flight()
                        .is_some_and(|drag| drag.widget == response.id)
                {
                    state.mark_drag_moved();
                }
            }

            if response.drag_stopped() {
                state.end_drag(response.id);
            }
        }
    }

    fn minimized_body(
        &mut self,
        ui: &mut Ui,
        surface_index: SurfaceIndex,
        fade_style: Option<&Style>,
        title: Atoms<'static>,
        tab_count: usize,
    ) {
        ui.horizontal(|ui| {
            let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
            // Read out before the expand button is drawn: that borrows `self` mutably, and the
            // row's height is wanted again below, for the title.
            let bar_height = style.tab_bar.height;
            let (tabbar_outer_rect, _) = ui.allocate_exact_size(
                vec2(Style::TAB_EXPAND_BUTTON_SIZE, bar_height),
                Sense::hover(),
            );
            ui.painter().rect_filled(
                tabbar_outer_rect,
                style.tab_bar.corner_radius,
                style.tab_bar.bg_fill,
            );
            self.window_expand(ui, surface_index, tabbar_outer_rect, fade_style);
            // One line high, so that a title carrying an icon reads as a row in this bar rather
            // than growing it to the icon's own resolution.
            ui.add(AtomLayout::new(title).max_height(bar_height));
            if tab_count > 1 {
                ui.label(
                    RichText::new(format!("+{}", tab_count - 1))
                        .color(ui.visuals().weak_text_color()),
                );
            }
            ui.allocate_space(ui.available_size());
        });
    }

    /// Draws the expand window button.
    fn window_expand(
        &mut self,
        ui: &mut Ui,
        surface_index: SurfaceIndex,
        tabbar_outer_rect: Rect,
        fade_style: Option<&Style>,
    ) {
        let rect = tabbar_outer_rect;

        let ui = &mut ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center))
                .id_salt((surface_index, "window_expand")),
        );

        let (rect, mut response) = ui.allocate_exact_size(ui.available_size(), Sense::click());

        response = response.on_hover_cursor(CursorIcon::PointingHand);

        let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
        let color = if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(
                rect,
                CornerRadius::ZERO,
                style.buttons.minimize_window_bg_fill,
            );
            style.buttons.minimize_window_active_color
        } else {
            style.buttons.minimize_window_color
        };

        let mut arrow_rect = rect;

        rect_set_size_centered(&mut arrow_rect, Vec2::splat(Style::TAB_EXPAND_ARROW_SIZE));

        Self::draw_chevron_right(ui, &mut response, style, color, arrow_rect);

        // Draw button right border.
        ui.painter().vline(
            rect.right(),
            rect.y_range(),
            Stroke::new(
                ui.ctx().pixels_per_point().recip(),
                style.buttons.minimize_window_border_color,
            ),
        );

        if response.clicked() {
            self.window_request_toggle_minimized(surface_index);
        }
    }

    fn draw_chevron_right(
        ui: &mut Ui,
        response: &mut Response,
        style: &Style,
        color: Color32,
        arrow_rect: Rect,
    ) {
        ui.painter().add(Shape::convex_polygon(
            // Arrow pointing rightwards.
            vec![
                arrow_rect.left_top(),
                arrow_rect.center(),
                arrow_rect.left_bottom(),
            ],
            color,
            Stroke::NONE,
        ));

        // Chevron pointing rightwards.
        ui.painter().add(Shape::convex_polygon(
            vec![
                arrow_rect.center_top(),
                arrow_rect.right_center(),
                arrow_rect.center_bottom(),
            ],
            color,
            Stroke::NONE,
        ));
        let color = if response.hovered() || response.has_focus() {
            style.buttons.minimize_window_bg_fill
        } else {
            style.tab_bar.bg_fill
        };
        ui.painter().add(Shape::convex_polygon(
            vec![
                arrow_rect
                    .center_top()
                    .lerp(arrow_rect.center_bottom(), 0.25),
                arrow_rect.center().lerp(arrow_rect.right_center(), 0.5),
                arrow_rect
                    .center_top()
                    .lerp(arrow_rect.center_bottom(), 0.75),
            ],
            color,
            Stroke::NONE,
        ));
    }

    /// Ask for a window to be minimized or restored — the click handler's half.
    ///
    /// Queued rather than done here for the reason the whole of `DockMutation` exists: the
    /// surface whose window this is, is the one being drawn. The value is computed now, off
    /// the state the click saw, so two requests in a frame cannot toggle each other away.
    pub(super) fn window_request_toggle_minimized(&mut self, surf_index: SurfaceIndex) {
        let minimized = self
            .dock_state
            .get_window_state(surf_index)
            .unwrap()
            .is_minimized();
        self.mutations.push(DockMutation::SetWindowMinimized {
            surface: surf_index,
            minimized: !minimized,
        });
    }

    /// Minimize or restore a window — the epilogue's half, applied from
    /// [`DockMutation::SetWindowMinimized`].
    ///
    /// Reads this pass's geometry to remember how tall the window was, exactly as it did when
    /// it ran during the click.
    pub(super) fn window_set_minimized(&mut self, surf_index: SurfaceIndex, minimized: bool) {
        let was_minimized = self
            .dock_state
            .get_window_state(surf_index)
            .unwrap()
            .is_minimized();
        if was_minimized == minimized {
            return;
        }
        let surface = &mut self.dock_state[surf_index];

        if surface.root_node().is_some_and(|node| node.is_collapsed()) {
            // The window is already fully collapsed,
            // so `expanded_height` has already been set.
            // We don't need to set `new` either.
            if let Some(window_state) = self.dock_state.get_window_state_mut(surf_index) {
                window_state.toggle_minimized();
            }
        } else if was_minimized {
            if let Some(window_state) = self.dock_state.get_window_state_mut(surf_index) {
                window_state.set_new(true);
                window_state.toggle_minimized();
            }
        } else {
            // Remember how tall the window was so un-minimizing restores that height. A
            // surface that was never laid out has no height to remember.
            let surface_height = self.dock_state[surf_index]
                .root()
                .and_then(|root| self.layout.rect(NodePath::new(surf_index, root)))
                .map_or(0.0, |rect| rect.height());
            if let Some(window_state) = self.dock_state.get_window_state_mut(surf_index) {
                window_state.set_expanded_height(surface_height);
                window_state.toggle_minimized();
            }
        }
        self.events.push(DockEvent::LayoutCommitted);
    }
}
