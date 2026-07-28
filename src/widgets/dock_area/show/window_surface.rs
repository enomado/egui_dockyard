use egui::{
    Align, Color32, CornerRadius, CursorIcon, Frame, Layout, Rect, Response, RichText, Sense,
    Shape, Stroke, Ui, UiBuilder, Vec2, WidgetText, vec2,
};

use crate::{
    DockArea, NodePath, Style, SurfaceIndex, TabViewer,
    dock_area::{events::DockEvent, state::State, tab_removal::TabRemoval},
    utils::{fade_visuals, rect_set_size_centered},
};

impl<Tab> DockArea<'_, Tab> {
    pub(super) fn show_window_surface(
        &mut self,
        ui: &Ui,
        surf_index: SurfaceIndex,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        state: &mut State,
        fade_style: Option<(&Style, f32, SurfaceIndex)>,
    ) {
        // Construct egui window
        let id = format!("window {surf_index:?}").into();
        let bounds = self.window_bounds.unwrap();
        let open = true;
        let window = crate::dock_area::window_ui::create_window(
            self.dock_state.get_window_state_mut(surf_index).unwrap(),
            id,
            bounds,
        );

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
            let leaf = self.dock_state[surf_index][node_id].get_leaf_mut().unwrap();
            let active = leaf
                .active_focused()
                .expect("a window surface should never hold an empty leaf");
            tab_viewer
                .title(active)
                .color(ui.visuals().widgets.noninteractive.fg_stroke.color)
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

        let tab_bar_height = self.style.as_ref().unwrap().tab_bar.height;
        let minimized = self
            .dock_state
            .get_window_state(surf_index)
            .unwrap()
            .is_minimized();
        if minimized {
            let height = tab_bar_height;
            window
                .resizable([true, false])
                .max_height(height)
                .min_height(height)
        } else if self.dock_state[surf_index].is_collapsed() {
            let height = self.dock_state[surf_index].collapsed_leaf_count() as f32 * tab_bar_height;
            window
                .resizable([true, false])
                .max_height(height)
                .min_height(height)
        } else {
            window
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

        if !open {
            self.to_remove.push(TabRemoval::Window(surf_index));
        }
    }

    fn minimized_body(
        &mut self,
        ui: &mut Ui,
        surface_index: SurfaceIndex,
        fade_style: Option<&Style>,
        title: WidgetText,
        tab_count: usize,
    ) {
        ui.horizontal(|ui| {
            let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
            let (tabbar_outer_rect, _) = ui.allocate_exact_size(
                vec2(Style::TAB_EXPAND_BUTTON_SIZE, style.tab_bar.height),
                Sense::hover(),
            );
            ui.painter().rect_filled(
                tabbar_outer_rect,
                style.tab_bar.corner_radius,
                style.tab_bar.bg_fill,
            );
            self.window_expand(ui, surface_index, tabbar_outer_rect, fade_style);
            ui.label(title);
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
            self.window_toggle_minimized(surface_index);
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

    pub(super) fn window_toggle_minimized(&mut self, surf_index: SurfaceIndex) {
        let minimized = self
            .dock_state
            .get_window_state(surf_index)
            .unwrap()
            .is_minimized();
        let surface = &mut self.dock_state[surf_index];

        if surface.root_node().is_some_and(|node| node.is_collapsed()) {
            // The window is already fully collapsed,
            // so `expanded_height` has already been set.
            // We don't need to set `new` either.
            if let Some(window_state) = self.dock_state.get_window_state_mut(surf_index) {
                window_state.toggle_minimized();
            }
        } else if minimized {
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
