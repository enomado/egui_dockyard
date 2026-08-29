use std::{f32::consts::FRAC_PI_2, ops::RangeInclusive, sync::Arc};

use egui::{
    Align, Align2, Button, Color32, CornerRadius, CursorIcon, Frame, Galley, Id, Key, LayerId,
    Layout, NumExt, Order, Popup, PopupCloseBehavior, Rect, Response, ScrollArea, Sense, Shape,
    Stroke, StrokeKind, TextStyle, TextWrapMode, Ui, UiBuilder, Vec2, WidgetText,
    emath::TSTransform, epaint::TextShape, pos2, vec2,
};

use crate::NodePath;
use crate::dock_area::ids::tab_widget_id;
use crate::dock_area::tab_removal::{ForcedRemoval, TabRemoval};
use crate::layout::SideStrip;
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

/// The least text a squeezed name needs before it stops being a name.
///
/// Truncation can always make a name fit, which is exactly why a lower bound is needed: without
/// one, forty tabs would be forty ellipses saying nothing at all. So the bound is what it takes
/// for a *name* to survive the squeeze — room for four or five characters ahead of the ellipsis,
/// enough that `Geology` comes out as `Geol…` rather than as the bare mark.
///
/// A tab bar and a strip both stop here; what differs is what each has to add around the text
/// (padding at both ends, and a close button in a tab).
const MIN_SQUEEZED_TEXT: f32 = 40.0;

/// The shortest a name in a strip may be squeezed before the strip gives up on it.
const STRIP_MIN_NAME_LENGTH: f32 = MIN_SQUEEZED_TEXT + 2.0 * STRIP_NAME_PADDING;

/// Breathing room at each end of a name, along the strip.
const STRIP_NAME_PADDING: f32 = 4.0;

/// Breathing room at each end of a tab's name, and how far its text sits from the tab's edge.
const TAB_TEXT_PADDING: f32 = 8.0;

/// What a strip or a tab bar draws in place of the names it had no room for.
const OVERFLOW_MARK: &str = "…";

/// Lays a strip's name out into `room` along the strip, truncating it with an ellipsis.
///
/// Pass `f32::INFINITY` to ask how long the name would like to be: `Truncate` only cuts at the
/// width it is given, so one call answers both "how much does this name want" and "draw it this
/// long", and the two can never disagree about where the cut falls.
fn strip_galley(ui: &Ui, title: WidgetText, room: f32) -> Arc<Galley> {
    title.into_galley(
        ui,
        Some(TextWrapMode::Truncate),
        (room - 2.0 * STRIP_NAME_PADDING).max(0.0),
        TextStyle::Button,
    )
}

/// The room a laid-out name takes along the strip: its glyphs, plus padding at each end.
fn strip_length(galley: &Galley) -> f32 {
    galley.size().x + 2.0 * STRIP_NAME_PADDING
}

/// The rectangle a run of `length` along the strip takes, starting `cursor` along it.
///
/// A strip runs down the screen and a bar runs across it: everything a strip draws is the full
/// width of one and the full height of the other, so only which axis is which differs.
fn strip_slot(rect: Rect, vertical: bool, cursor: f32, length: f32) -> Rect {
    if vertical {
        Rect::from_min_size(pos2(rect.left(), cursor), vec2(rect.width(), length))
    } else {
        Rect::from_min_size(pos2(cursor, rect.top()), vec2(length, rect.height()))
    }
}

/// Draws `galley` in the middle of `slot`, turned a quarter turn when the strip is vertical.
///
/// Anchored at the middle of the galley, so the turn happens about the text's own centre and
/// lands it in the middle of the rectangle either way. Anticlockwise (`angle` counts clockwise),
/// which is what makes the glyphs run bottom-to-top — the direction a side bar is read in.
fn strip_text(ui: &Ui, slot: Rect, galley: Arc<Galley>, color: Color32, vertical: bool) {
    let position = slot.center() - galley.size() / 2.0;
    let mut text = TextShape::new(position, galley, color);
    if vertical {
        text = text.with_angle_and_anchor(-FRAC_PI_2, Align2::CENTER_CENTER);
    }
    ui.painter().add(text);
}

/// How the names of a strip share the room it has.
struct StripFit {
    /// What each drawn name gets, in list order. Shorter than the list of names when the strip
    /// could not hold them all even squeezed — the rest are what `overflow` stands for.
    lengths: Vec<f32>,
    /// Whether an ellipsis follows the names, standing for those that got no room at all.
    overflow: bool,
}

/// Shares `available` out between names wanting `naturals`, each behind a fixed gap of `gaps`.
///
/// Two rules, in this order, and both of them answers to "the strip is shorter than its names":
///
/// 1. **Squeeze every name before dropping any.** The room goes round as evenly as the names
///    allow: one shorter than its share keeps its own length and hands the difference back, so a
///    single long title cannot starve four short ones. A name given less than it wants is drawn
///    truncated, which is what says on screen that it was cut.
/// 2. **What cannot be squeezed in is stood for by one ellipsis** — never by silence. A strip
///    that simply stopped would be claiming the tabs past that point are not there. Names are
///    dropped from the end, keeping the tree's order, once even [`STRIP_MIN_NAME_LENGTH`] apiece
///    is more than the strip has.
///
/// `naturals` and `gaps` run in step; `ellipsis` is the room the ellipsis itself needs.
fn fit_strip_names(naturals: &[f32], gaps: &[f32], available: f32, ellipsis: f32) -> StripFit {
    debug_assert_eq!(naturals.len(), gaps.len());

    // How many names get drawn at all. Each costs its gap plus the least it can be squeezed
    // into — which for a name already shorter than the minimum is its own length, so a column of
    // short names is not thinned out to honour a minimum none of them needs. While names are
    // still left over, the ellipsis has to be paid for out of the same length.
    let mut shown = 0;
    let mut spent = 0.0;
    while shown < naturals.len() {
        let cost = gaps[shown] + naturals[shown].min(STRIP_MIN_NAME_LENGTH);
        let tail = if shown + 1 < naturals.len() {
            ellipsis
        } else {
            0.0
        };
        if spent + cost + tail > available {
            break;
        }
        spent += cost;
        shown += 1;
    }

    // Unless the strip is too short even for the ellipsis, in which case there is nothing honest
    // left to draw — and drawing a cut-off ellipsis would be the same lie in smaller print.
    let overflow = shown < naturals.len() && ellipsis <= available;

    let mut budget = available - gaps[..shown].iter().sum::<f32>();
    if overflow {
        budget -= ellipsis;
    }
    let budget = budget;

    StripFit {
        lengths: share_room(&naturals[..shown], budget),
        overflow,
    }
}

/// The mark a tab bar draws when it is not showing every tab it has.
fn tab_mark_galley(ui: &Ui) -> Arc<Galley> {
    WidgetText::from(OVERFLOW_MARK).into_galley(ui, None, f32::INFINITY, TextStyle::Button)
}

/// How wide each tab of a bar gets to be, and whether the bar has to admit it is not showing
/// all of them.
struct TabBarFit {
    /// One width per tab, in bar order. Every tab gets one: a tab bar drops nothing, because it
    /// scrolls, and a tab that scrolled off is still reachable.
    widths: Vec<f32>,
    /// Set when the tabs do not fit even squeezed. The bar then keeps `mark` px at its right end
    /// for the ellipsis, which says there is more here than is on screen.
    mark: Option<f32>,
}

/// Shares `available` out between tabs wanting `wants`, never squeezing one below its `floor`.
///
/// A tab bar squeezes for the same reason a strip does, and stops for a different one. Where a
/// strip runs out of room it *drops* names, because there is nothing else it could do with them;
/// a bar scrolls, so every tab keeps a width and what does not fit stays reachable by the wheel.
/// The mark at the end is what says so — without it a bar that is scrolled to the left looks
/// exactly like a bar with nothing more to show.
///
/// `fixed` is the room the gaps between tabs take, which no tab can be given. `ellipsis` is what
/// the mark itself needs; it is charged to the same width, so the mark never lands on a tab.
fn fit_tab_widths(
    wants: &[f32],
    floors: &[f32],
    fixed: f32,
    available: f32,
    ellipsis: f32,
) -> TabBarFit {
    debug_assert_eq!(wants.len(), floors.len());

    // `floor` is a floor, not a width: a tab whose name is shorter than the minimum keeps its own
    // width instead of being padded out to a minimum it does not need.
    let widths: Vec<f32> = share_room(wants, available - fixed)
        .into_iter()
        .zip(floors)
        .map(|(share, floor)| share.max(*floor))
        .collect();

    // `share_room` never hands out more than the budget, so the only way a bar overflows is a
    // floor holding a tab up — which is also why the mark is not paid for out of the shares.
    // Reducing the budget would move the tabs that are *above* their floor, and at this point
    // there are none: it is the floors that are over the budget. The room for the mark comes off
    // the width the bar shows instead, in `tab_bar`, so it never lands on a tab.
    let mark = (widths.iter().sum::<f32>() + fixed > available).then_some(ellipsis);
    TabBarFit { widths, mark }
}

/// Shares `budget` out between claims wanting `naturals`, evenly but never past what each wants.
///
/// Water filling: shortest claim first, each taking an equal share of what is left or its own
/// length, whichever is less. Handing a short claim's surplus back to those still waiting is what
/// makes the result even — one long name cannot starve four short ones — and what keeps a short
/// name from being padded out past its own text.
///
/// Claims may come back with less than they asked for; that is the whole point, and what the
/// caller does about it (truncate, drop, leave to a scrollbar) is the caller's own rule.
fn share_room(naturals: &[f32], budget: f32) -> Vec<f32> {
    let mut order: Vec<usize> = (0..naturals.len()).collect();
    order.sort_by(|left, right| naturals[*left].total_cmp(&naturals[*right]));

    let mut shares = vec![0.0; naturals.len()];
    let mut left = budget;
    let mut waiting = naturals.len();
    for index in order {
        shares[index] = naturals[index].min(left / waiting as f32);
        left -= shares[index];
        waiting -= 1;
    }
    shares
}

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
    title: WidgetText,
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
                DockMutation::SetLeafCollapsed {
                    path,
                    collapsed: false,
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

        // How the width is shared out among the tabs, worked out before any of them is drawn —
        // and, when they do not all fit, the room the mark saying so takes at the right end.
        let fit = self.tab_widths(ui, path, tab_viewer, fade_style, available_width);
        if let Some(mark) = fit.mark {
            available_width -= mark;
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

            let tab_hovered = self.tabs(
                tabs_ui,
                state,
                path,
                tab_viewer,
                tabbar_outer_rect,
                &fit.widths,
                fade_style,
            );

            // The mark for the tabs that did not fit, in the room reserved for it above so that
            // it never lands on a tab. Outside `tabs_ui`, and so outside the scroll: it says the
            // bar is not showing everything it has, which stays true wherever the bar is
            // scrolled to. A mark and not a tab — there is no one tab behind it to click.
            if let Some(mark) = fit.mark {
                let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
                let galley = tab_mark_galley(ui);
                let slot = Rect::from_min_size(
                    pos2(clip_rect.right(), tabbar_outer_rect.top()),
                    vec2(mark, tabbar_outer_rect.height()),
                );
                ui.painter().add(TextShape::new(
                    slot.center() - galley.size() / 2.0,
                    galley,
                    style.tab.inactive.text_color,
                ));
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
                    DockMutation::SetLeafCollapsed {
                        path,
                        collapsed: !collapsed,
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

        let mut wants = Vec::with_capacity(leaf.len());
        let mut floors = Vec::with_capacity(leaf.len());
        let mut fixed = 0.0;
        for index in 0..leaf.len() {
            let tab_index = TabIndex(index);
            let tab = &leaf[tab_index];
            let tab_style = tab_viewer
                .tab_style_override(tab, &style.tab)
                .unwrap_or_else(|| style.tab.clone());

            let close = if self.show_close_buttons && tab_viewer.is_closeable(tab) {
                Style::TAB_CLOSE_BUTTON_SIZE.min(style.tab_bar.height)
            } else {
                0.0
            };
            let furniture = close + 2.0 * TAB_TEXT_PADDING;
            let text = tab_viewer
                .title(tab)
                .into_galley(ui, None, f32::INFINITY, TextStyle::Button)
                .size()
                .x;

            let want = (text + furniture).at_least(tab_style.minimum_width.unwrap_or(0.0));
            wants.push(want);
            floors.push(want.min(furniture + MIN_SQUEEZED_TEXT));
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
                *want = want.at_least(equal);
            }
        }

        let ellipsis = tab_mark_galley(ui).size().x + 2.0 * TAB_TEXT_PADDING;
        fit_tab_widths(&wants, &floors, fixed, available_width, ellipsis)
    }

    #[allow(clippy::too_many_arguments)]
    fn tabs(
        &mut self,
        tabs_ui: &mut Ui,
        state: &mut State,
        path: NodePath,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        tabbar_outer_rect: Rect,
        widths: &[f32],
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
                            widths[tab_index.0],
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
                    widths[tab_index.0],
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
            ui, path, tab_viewer, fade_style, side, names_rect, on_toggle,
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
        path: NodePath,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        fade_style: Option<&Style>,
        side: Option<SideStrip>,
        rect: Rect,
        expand: DockMutation,
    ) {
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
        let names = self.strip_name_list(tab_viewer, path, fade_style);
        if names.is_empty() {
            return;
        }

        let style = fade_style.unwrap_or_else(|| self.style.as_ref().unwrap());
        let hairline = style.separator.color_idle;
        // The ellipsis is not a tab and cannot be clicked, so it is drawn in the plainest of the
        // three tab colours rather than in one that would offer something it does not have.
        let overflow_color = style.tab.inactive.text_color;
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
            .map(|name| strip_length(&strip_galley(ui, name.title.clone(), f32::INFINITY)))
            .collect();
        let overflow = strip_galley(ui, OVERFLOW_MARK.into(), f32::INFINITY);
        let fit = fit_strip_names(&naturals, &gaps, end - start, strip_length(&overflow));

        let mut cursor = start;
        for (index, name) in names.into_iter().take(fit.lengths.len()).enumerate() {
            if gaps[index] > 0.0 {
                let separator = strip_slot(rect, vertical, cursor, gaps[index]);
                ui.painter().rect_filled(separator, 0.0, hairline);
                cursor += gaps[index];
            }

            let galley = strip_galley(ui, name.title, fit.lengths[index]);
            let length = strip_length(&galley);
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

            strip_text(ui, name_rect, galley, tab_style.text_color, vertical);

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
            let slot = strip_slot(rect, vertical, cursor, strip_length(&overflow));
            strip_text(ui, slot, overflow, overflow_color, vertical);
        }
    }

    /// What a strip at `path` has to name, in the order it will come back in.
    ///
    /// One leaf's tabs, or — for a side stowed as a unit — every leaf inside it, depth first and
    /// first child first, which is top-to-bottom and left-to-right on screen.
    fn strip_name_list(
        &mut self,
        tab_viewer: &mut impl TabViewer<Tab = Tab>,
        path: NodePath,
        fade_style: Option<&Style>,
    ) -> Vec<StripName> {
        let mut leaves = Vec::new();
        let mut stack = vec![path.node];
        while let Some(node) = stack.pop() {
            if self.dock_state[path.surface][node].is_leaf() {
                leaves.push(NodePath::new(path.surface, node));
            } else if let Some([left, right]) = self.dock_state[path.surface].children(node) {
                // Pushed back to front: the stack hands the first child back first.
                stack.push(right);
                stack.push(left);
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
                    title: tab_viewer.title(&leaf[tab_index]),
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
        } else if stow_target.is_some() {
            Self::draw_stow_arrow(ui, color, arrow_rect);
        } else if let Some(side) = side_strip {
            Self::draw_side_arrow(side, ui, color, arrow_rect);
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
            } else if let Some(target) = stow_target {
                self.mutations.push(DockMutation::SetSplitStowed {
                    path: target,
                    stowed: true,
                });
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
    /// [`None`] where the gesture would add nothing. A leaf that is *itself* a side already goes
    /// away with the plain arrow, which folds it into a strip — the same picture, one modifier
    /// less; and a side already stowed is what the plain arrow brings back.
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

    /// The arrow of "put my whole side away": the ordinary collapse triangle, doubled.
    ///
    /// Doubled rather than a glyph of its own, because the gesture is not a different action —
    /// it is the same fold one level up, and the icon says so: what the plain arrow does to this
    /// leaf, this one does to everything beside it.
    fn draw_stow_arrow(ui: &mut Ui, color: Color32, arrow_rect: Rect) {
        // Two triangles stacked along the arrow's own axis, each half as tall, with a pixel
        // between them so they read as two at the size this is drawn.
        let half = arrow_rect.height() * 0.5;
        for step in 0..2 {
            let top = arrow_rect.top() + half * step as f32;
            let cell = Rect::from_min_max(
                pos2(arrow_rect.left(), top),
                pos2(arrow_rect.right(), top + half - 1.0),
            );
            ui.painter().add(Shape::convex_polygon(
                vec![cell.left_top(), cell.right_top(), cell.center_bottom()],
                color,
                Stroke::NONE,
            ));
        }
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

    /// The arrow of a sideways collapsed strip, pointing at the space the leaf will expand
    /// into — which is away from the edge it is pressed against, and therefore the one thing
    /// the strip's own side has to be known for.
    fn draw_side_arrow(side: SideStrip, ui: &mut Ui, color: Color32, arrow_rect: Rect) {
        ui.painter().add(Shape::convex_polygon(
            match side {
                // Against the left edge: expands rightwards.
                SideStrip::Left => vec![
                    arrow_rect.left_top(),
                    arrow_rect.right_center(),
                    arrow_rect.left_bottom(),
                ],
                // Against the right edge: expands leftwards.
                SideStrip::Right => vec![
                    arrow_rect.right_top(),
                    arrow_rect.left_center(),
                    arrow_rect.right_bottom(),
                ],
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
        tab_width: f32,
        show_close_button: bool,
        fade: Option<&Style>,
    ) -> (Response, Option<Response>) {
        let style = fade.unwrap_or_else(|| self.style.as_ref().unwrap());
        let x_spacing = TAB_TEXT_PADDING;
        let close_button_size = if show_close_button {
            Style::TAB_CLOSE_BUTTON_SIZE.min(style.tab_bar.height)
        } else {
            0.0
        };

        // The width was decided for the bar as a whole (`tab_widths`), so the name is laid out
        // into what this tab was given rather than the other way round: `Truncate` cuts it with
        // an ellipsis when the bar had to squeeze, and changes nothing when it did not.
        let galley = label.into_galley(
            ui,
            Some(TextWrapMode::Truncate),
            (tab_width - close_button_size - 2.0 * x_spacing).max(0.0),
            TextStyle::Button,
        );

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
    use super::{STRIP_MIN_NAME_LENGTH, fit_strip_names, fit_tab_widths, tab_body_id};
    use crate::{DockState, NodePath, SurfaceIndex};
    use egui::Id;

    /// What an ellipsis costs, near enough: these are tests of the sharing, not of a font.
    const ELLIPSIS: f32 = 12.0;

    fn no_gaps(count: usize) -> Vec<f32> {
        vec![0.0; count]
    }

    /// Every name is squeezed before any of them is dropped: three names wanting 200 apiece get
    /// a third of the strip each, rather than the first one taking it and the last going missing.
    #[test]
    fn a_short_strip_squeezes_its_names_rather_than_dropping_them() {
        let fit = fit_strip_names(&[200.0, 200.0, 200.0], &no_gaps(3), 300.0, ELLIPSIS);

        assert_eq!(fit.lengths, vec![100.0, 100.0, 100.0]);
        assert!(!fit.overflow, "all three names fit, squeezed");
    }

    /// A name shorter than its share keeps its own length and hands the difference back.
    ///
    /// Splitting the room evenly would give the short name 70 px it cannot use and leave the two
    /// long ones 30 px shorter each for nothing.
    #[test]
    fn a_short_name_gives_its_surplus_to_the_others() {
        let fit = fit_strip_names(&[10.0, 200.0, 200.0], &no_gaps(3), 210.0, ELLIPSIS);

        assert_eq!(fit.lengths, vec![10.0, 100.0, 100.0]);
    }

    /// A column of names that are all short is not thinned out to honour a minimum none of them
    /// needs: what a name costs the strip is its own length when that is less than the minimum.
    #[test]
    fn short_names_are_not_dropped_to_honour_the_minimum() {
        let naturals = vec![10.0; 20];
        let fit = fit_strip_names(&naturals, &no_gaps(20), 210.0, ELLIPSIS);

        assert_eq!(fit.lengths, naturals, "all twenty fit at their own length");
        assert!(!fit.overflow);
    }

    /// What cannot be squeezed in is stood for by an ellipsis, and the room it needs comes out of
    /// the same length rather than being taken on top of it.
    #[test]
    fn what_will_not_fit_is_stood_for_by_an_ellipsis() {
        let available = 4.5 * STRIP_MIN_NAME_LENGTH;
        let fit = fit_strip_names(&[200.0; 10], &no_gaps(10), available, ELLIPSIS);

        assert!(
            fit.overflow,
            "ten names of 200 px cannot fit in {available}"
        );
        assert_eq!(fit.lengths.len(), 4, "as many as the minimum allows");
        let drawn: f32 = fit.lengths.iter().sum();
        assert!(
            drawn + ELLIPSIS <= available,
            "the ellipsis has to fit too: {drawn} + {ELLIPSIS} > {available}"
        );
    }

    /// A strip too short even for the ellipsis draws nothing: a cut-off ellipsis would be the
    /// same lie in smaller print.
    #[test]
    fn a_strip_too_short_for_the_ellipsis_says_nothing() {
        let fit = fit_strip_names(&[200.0, 200.0], &no_gaps(2), ELLIPSIS / 2.0, ELLIPSIS);

        assert!(fit.lengths.is_empty());
        assert!(!fit.overflow);
    }

    /// A bar shares its width out between the tabs rather than serving them in order until it
    /// runs out: three tabs wanting 300 px each get a third of the bar apiece.
    #[test]
    fn a_full_bar_squeezes_its_tabs() {
        let fit = fit_tab_widths(&[300.0; 3], &[72.0; 3], 0.0, 300.0, ELLIPSIS);

        assert_eq!(fit.widths, vec![100.0, 100.0, 100.0]);
        assert!(fit.mark.is_none(), "squeezed, but all three are on screen");
    }

    /// No tab is squeezed past the point where its name stops being a name, even if that is what
    /// it would take to fit them all — the bar scrolls, so the tabs past the edge are not lost.
    #[test]
    fn a_tab_is_not_squeezed_below_its_floor() {
        let fit = fit_tab_widths(&[300.0, 300.0], &[72.0, 72.0], 0.0, 100.0, ELLIPSIS);

        assert_eq!(fit.widths, vec![72.0, 72.0], "held up by the floor");
        assert_eq!(
            fit.mark,
            Some(ELLIPSIS),
            "144 px of tabs in a 100 px bar: the bar has to say so"
        );
    }

    /// A tab whose name is short keeps its own width instead of being padded to the floor, and
    /// hands what it does not need to the tab beside it.
    #[test]
    fn a_short_tab_keeps_its_own_width() {
        let fit = fit_tab_widths(&[30.0, 300.0], &[30.0, 72.0], 0.0, 200.0, ELLIPSIS);

        assert_eq!(fit.widths, vec![30.0, 170.0]);
        assert!(fit.mark.is_none());
    }

    /// A bar with room to spare gives every tab what it asked for and says nothing.
    #[test]
    fn a_bar_with_room_to_spare_marks_nothing() {
        let fit = fit_tab_widths(&[100.0, 100.0], &[72.0, 72.0], 0.0, 400.0, ELLIPSIS);

        assert_eq!(fit.widths, vec![100.0, 100.0], "nothing to squeeze");
        assert!(fit.mark.is_none());
    }

    /// The gaps between tabs are not the tabs' to share: they come off the width first.
    #[test]
    fn the_gaps_between_tabs_are_not_shared_out() {
        let fit = fit_tab_widths(&[300.0, 300.0], &[72.0, 72.0], 20.0, 220.0, ELLIPSIS);

        assert_eq!(fit.widths, vec![100.0, 100.0]);
        assert!(
            fit.mark.is_none(),
            "200 px of tabs and 20 px of gap fit a 220 px bar exactly"
        );
    }

    /// The hairlines between leaves come out of the strip's length like everything else.
    #[test]
    fn a_gap_is_paid_for_out_of_the_strip() {
        let available = 101.0;
        let fit = fit_strip_names(&[100.0, 100.0], &[0.0, 1.0], available, ELLIPSIS);

        assert_eq!(fit.lengths, vec![50.0, 50.0]);
        assert!(!fit.overflow);
        let drawn: f32 = fit.lengths.iter().sum::<f32>() + 1.0;
        assert!(drawn <= available, "{drawn} px drawn into {available} px");
    }

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
