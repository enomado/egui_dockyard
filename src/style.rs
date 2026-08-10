use egui::{CornerRadius, Margin, Stroke, ecolor::*};

/// Left or right alignment for tab add button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[allow(missing_docs)]
pub enum TabAddAlign {
    Left,
    Right,
}

/// Lets you change how tabs and the [`DockArea`](crate::DockArea) should look and feel.
/// [`Style`] is divided into several, more specialized structs that handle individual
/// elements of the UI.
///
/// Your [`Style`] can inherit all its properties from an [`egui::Style`] through the
/// [`Style::from_egui`] function.
///
/// # It is a save format, and it is versionless
///
/// Under the `serde` feature every struct here carries `#[serde(default)]` at the *container*
/// level, so a style saved by an older version of this crate loads into a newer one: fields it
/// has never heard of take their [`Default`], and fields it no longer has are ignored. That is
/// deliberate, and it is a promise rather than an accident of how the structs happen to be
/// written.
///
/// It began as one attribute on one field — `cross_split_toggle`, added to a struct consumers
/// were already persisting — which left the next person adding a field with the same problem
/// and no precedent visible among its neighbours. Answering it per-field is answering it once
/// per field, forever; answering it per-struct means a new field needs nothing at all, which is
/// the only version of the rule that survives being forgotten.
///
/// The other half of the promise is what a missing field falls back to: the struct's own
/// `Default`, *not* [`Style::from_egui`]. A style loaded from an old save is therefore a mix of
/// what was saved and this crate's defaults, and never of the host's egui theme.
///
/// Example:
///
/// ```rust
/// # use egui_dock::{DockArea, DockState, OverlayType, Style, TabAddAlign, TabViewer};
/// # use egui::{Ui, WidgetText};
/// # struct MyTabViewer;
/// # impl TabViewer for MyTabViewer {
/// #     type Tab = ();
/// #     fn title(&mut self, tab: &mut Self::Tab) -> WidgetText { WidgetText::default() }
/// #     fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {}
/// # }
/// # egui::__run_test_ui(|ui| {
/// # #[allow(deprecated)]
/// # egui::CentralPanel::default().show(ui, |ui| {
/// # let mut dock_state = DockState::new(vec![]);
/// // Inherit the look and feel from egui.
/// let mut style = Style::from_egui(ui.style());
///
/// // Modify a few fields.
/// style.overlay.overlay_type = OverlayType::HighlightedAreas;
/// style.buttons.add_tab_align = TabAddAlign::Left;
///
/// // Use the style with the `DockArea`.
/// DockArea::new(&mut dock_state)
///     .style(style)
///     .show_inside(ui, &mut MyTabViewer);
/// # });
/// # });
/// #
/// ```
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
#[allow(missing_docs)]
pub struct Style {
    /// Sets padding to indent from the edges of the window. By `Default` it's `None`.
    pub dock_area_padding: Option<Margin>,

    pub main_surface_border_stroke: Stroke,
    pub main_surface_border_rounding: CornerRadius,

    pub buttons: ButtonsStyle,
    pub separator: SeparatorStyle,
    pub cross_split_toggle: CrossSplitToggleStyle,
    pub tab_bar: TabBarStyle,
    pub tab: TabStyle,
    pub overlay: OverlayStyle,
}

/// Specifies the look and feel of buttons.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct ButtonsStyle {
    /// Color of the close tab button.
    pub close_tab_color: Color32,

    /// Color of the active close tab button.
    pub close_tab_active_color: Color32,

    /// Color of the background close tab button.
    pub close_tab_bg_fill: Color32,

    /// Left or right aligning of the add tab button.
    pub add_tab_align: TabAddAlign,

    /// Color of the add tab button.
    pub add_tab_color: Color32,

    /// Color of the active add tab button.
    pub add_tab_active_color: Color32,

    /// Color of the add tab button's background.
    pub add_tab_bg_fill: Color32,

    /// Color of the add tab button's left border.
    pub add_tab_border_color: Color32,

    /// Color of the close all tabs button.
    pub close_all_tabs_color: Color32,

    /// Color of the active close all tabs button.
    pub close_all_tabs_active_color: Color32,

    /// Color of the close all tabs button's background.
    pub close_all_tabs_bg_fill: Color32,

    /// Color of the close all tabs button's left border.
    pub close_all_tabs_border_color: Color32,

    /// Color of disabled close all tabs button.
    pub close_all_tabs_disabled_color: Color32,

    /// Color of the collapse tabs button.
    pub collapse_tabs_color: Color32,

    /// Color of the active collapse tabs button.
    pub collapse_tabs_active_color: Color32,

    /// Color of the collapse tabs button's background.
    pub collapse_tabs_bg_fill: Color32,

    /// Color of the collapse tabs button's left border.
    pub collapse_tabs_border_color: Color32,

    /// Color of the minimize window button.
    pub minimize_window_color: Color32,

    /// Color of the active minimize window button.
    pub minimize_window_active_color: Color32,

    /// Color of the minimize window button's background.
    pub minimize_window_bg_fill: Color32,

    /// Color of the minimize window button's left border.
    pub minimize_window_border_color: Color32,
}

/// Specifies the look and feel of node separators.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct SeparatorStyle {
    /// Width of the rectangle separator between nodes. By `Default` it's `1.0`.
    pub width: f32,

    /// Extra width added to the "logical thickness" of the rectangle so it's
    /// easier to grab. By `Default` it's `4.0`.
    pub extra_interact_width: f32,

    /// Limit for the allowed area for the separator offset. By `Default` it's `175.0`.
    /// `bigger value > less allowed offset` for the current window size.
    pub extra: f32,

    /// Idle color of the rectangle separator. By `Default` it's [`Color32::BLACK`].
    pub color_idle: Color32,

    /// Hovered color of the rectangle separator. By `Default` it's [`Color32::GRAY`].
    pub color_hovered: Color32,

    /// Dragged color of the rectangle separator. By `Default` it's [`Color32::WHITE`].
    pub color_dragged: Color32,
}

/// Geometry of the handle offered where separators meet: the square drawn at a junction, which
/// drags every separator meeting there at once and, at a crossing, swaps the grouping on
/// ctrl+click.
///
/// It is only ever on screen **under the pointer** — one is offered at every junction of every
/// line, and painted cold they would be a grid of squares over the panels that also made every
/// line harder to grab. Shape makes no difference to that: a crossing's handle used to need ctrl
/// held, back when its only gesture was the transposing click, and a crossing is dragged now.
///
/// Colors are not here — the handle is drawn in the separator's own palette
/// ([`SeparatorStyle::color_hovered`] and [`SeparatorStyle::color_dragged`], swapped between
/// square and icon while it is held), because it *is* part of the separator as far as the eye
/// is concerned. What this struct carries is the one thing the handle cannot inherit: how big
/// a target it is.
///
/// # Why there are two margins
///
/// The button sits on top of a separator a couple of points wide, so missing it is not a
/// no-op — the press lands on the separator underneath and starts a resize drag instead. It
/// therefore catches the pointer from outside the square it is drawn in (`catch_extra`), and,
/// once it has caught it, holds on from further out still (`hold_extra`). Without that second,
/// wider radius a pointer resting near the edge of the first one flips between "toggle" and
/// "resize the separator" on every pixel of jitter: the cursor, the highlight and the meaning
/// of a click all change with it.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct CrossSplitToggleStyle {
    /// Side of the square the button is drawn as, in points. By `Default` it's `10.0`.
    ///
    /// One size, hovered or not: the margins below widen what answers to the pointer, never
    /// what is painted. A button that grew under the cursor was tried and taken back out —
    /// at the default margins it more than doubled, and a control that changes size on
    /// approach reads as a thing being dragged, not as a thing being offered.
    pub size: f32,

    /// How far outside that square the pointer is still caught, in points. By `Default` it's
    /// `1.0`.
    ///
    /// One point, deliberately, and it was `6.0` two revisions ago: a handle that answers well
    /// outside itself is a handle that takes the separator's own grab zone away, and on a dock with
    /// a junction every few hundred points that reads as the lines being hard to grab —
    /// «область у Т слишком большая слишком назойливая», then «надо еще меньше — в два три раза»
    /// (Стас, 2026-08-10). What it is *for* is missing the square by a pixel, not aiming near it.
    pub catch_extra: f32,

    /// How far outside the catch zone the pointer is held once caught, in points. By `Default`
    /// it's `0.5`. Setting it to `0.0` removes the hysteresis.
    ///
    /// Trimmed with `catch_extra` and for the same reason. It is the hysteresis and not the reach:
    /// what it buys is that a hand resting on the edge of the catch zone does not flip between
    /// "handle" and "separator" on every pixel of jitter, and half a point buys that at the one
    /// place it matters — the boundary itself.
    pub hold_extra: f32,

    /// How far apart the two dividers may sit and still be offered a button, in points. By
    /// `Default` it's `8.0`.
    ///
    /// This is the magnet's reach, and it is a different question from the two margins above:
    /// they are about missing a button *with the pointer*, this one is about whether the button
    /// is there at all. Two dividers a few points out of line are a cross a hand meant to make
    /// and did not quite; the button is what offers to close the gap, so a tolerance that only
    /// admitted pairs already on the same pixel offered it exactly where it was not needed.
    ///
    /// The price is stated plainly, because there is one: a transposition averages the pair, so
    /// pressing the button on a pair `d` points apart moves each of them by `d / 2`. That is the
    /// magnet doing its job — the two lines come out as one — but it is a movement, and it grows
    /// with this number. It is also what bounds it: at some width the "+" is drawn on a jog
    /// visible enough that a press reads as the layout jumping rather than snapping.
    ///
    /// Floored at one device pixel, so `0.0` still means "the same line" rather than "bit-exact",
    /// and means it identically at every `pixels_per_point` — see `Crossings::tolerance`.
    pub align_tolerance: f32,

    /// Colour of the arrows drawn inside the square. By `Default` it's `Color32::from_gray(27)` —
    /// egui's own dark-theme panel fill — and [`Style::from_egui`] takes the host's
    /// [`egui::Visuals::panel_fill`] instead, so the icon reads as a *cut-out* of the square.
    ///
    /// It has to be its own colour, and that was found on the screen rather than reasoned out. The
    /// square and the icon used to take the two ends of the separator's palette, swapped depending
    /// on whether the handle was held: [`SeparatorStyle::color_hovered`] and
    /// [`SeparatorStyle::color_dragged`]. Under [`Style::from_egui`] those are
    /// `widgets.hovered.fg_stroke` and `widgets.active.fg_stroke`, which in egui's dark theme are
    /// **gray(240) and white** — so the handle came out as a plain white square with no arrows
    /// visible at all («кнопка стала полностью белой рисоваться без рисок», Стас, 2026-08-10). Two
    /// roles, one colour: the palette had no third end to give.
    ///
    /// The panel fill is the honest choice for it, because a theme already promises that its
    /// `fg_stroke` colours are legible *on* that fill — that is what they are for. So whatever the
    /// square takes, the arrows stay readable, in a light theme as in a dark one, without this
    /// having to inspect the square's colour and guess a contrast.
    pub icon_color: Color32,
}

impl CrossSplitToggleStyle {
    /// Derives relevant fields from `egui::Style` and sets the remaining fields to their default
    /// values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`CrossSplitToggleStyle::icon_color`]
    ///
    /// The geometry is not: how big a target a handle is, and how far it catches from, are this
    /// crate's own answers and have nothing in an egui theme to read them off.
    pub fn from_egui(style: &egui::Style) -> Self {
        Self {
            // The surface the theme's own `fg_stroke` colours are meant to be legible on, which is
            // exactly the promise the icon needs — see the field's doc.
            icon_color: style.visuals.panel_fill,
            ..Self::default()
        }
    }

    /// The width of the button in its widest form: the drawn square plus both margins, which is
    /// the zone it answers to while it is holding the pointer.
    ///
    /// Public because it is the one number about the button that anything *outside* the button
    /// needs — a harness aiming a press at a divider has to know what the button covers, so as
    /// not to press it by accident. It is arithmetic, not policy: re-deriving it elsewhere is
    /// how a copy goes stale, and one already had (it still said
    /// `(width + extra_interact_width).max(14.0)` a release after the magnet landed).
    ///
    /// What a crossing actually gets is this, shrunk to the room it has — see
    /// `Crossings::room_at` and `DockArea::toggle_room` — so this is an upper bound on screen,
    /// never an equality.
    pub fn widest(&self) -> f32 {
        self.size + 2.0 * (self.catch_extra + self.hold_extra)
    }
}

/// Specifies the look and feel of tab bars.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct TabBarStyle {
    /// Background color of tab bar. By `Default` it's [`Color32::WHITE`].
    pub bg_fill: Color32,

    /// Height of the tab bar. By `Default` it's `24.0`.
    pub height: f32,

    /// Inner margin of tab bar. By `Default` it's `Margin::ZERO`.
    pub inner_margin: Margin,

    /// Show a scroll bar when tab bar overflows. By `Default` it's `true`.
    pub show_scroll_bar_on_overflow: bool,

    /// Tab corner_radius. By `Default` it's [`CornerRadius::default`].
    pub corner_radius: CornerRadius,

    /// Color of the line separating the tab name area from the tab content area.
    /// By `Default` it's [`Color32::BLACK`].
    pub hline_color: Color32,

    /// Whether tab titles expand to fill the width of their tab bars.
    /// By `Default` it's `false`.
    pub fill_tab_bar: bool,
}

/// Specifies the look and feel of an individual tab.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct TabStyle {
    /// Style of the tab when it is active.
    pub active: TabInteractionStyle,

    /// Style of the tab when it is inactive.
    pub inactive: TabInteractionStyle,

    /// Style of the tab when it is focused.
    pub focused: TabInteractionStyle,

    /// Style of the tab when it is hovered.
    pub hovered: TabInteractionStyle,

    /// Style of the tab when it is inactive and has keyboard focus.
    pub inactive_with_kb_focus: TabInteractionStyle,

    /// Style of the tab when it is active and has keyboard focus.
    pub active_with_kb_focus: TabInteractionStyle,

    /// Style of the tab when it is focused and has keyboard focus.
    pub focused_with_kb_focus: TabInteractionStyle,

    /// Style for the tab body.
    pub tab_body: TabBodyStyle,

    /// If `true`, show the hline below the active tabs name.
    /// If `false`, show the active tab as merged with the tab ui area.
    /// By `Default` it's `false`.
    pub hline_below_active_tab_name: bool,

    /// Spacing between tabs.
    pub spacing: f32,

    /// The minimum width of the tab.
    ///
    /// The tab title or [`TabBarStyle::fill_tab_bar`] may make the tab
    /// wider than this but never shorter.
    pub minimum_width: Option<f32>,
}

/// Specifies the look and feel of individual tabs while they are being interacted with.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct TabInteractionStyle {
    /// Color of the outline around tabs. By `Default` it's [`Color32::BLACK`].
    pub outline_color: Color32,

    /// Tab corner radius. By `Default` it's [`CornerRadius::default`].
    pub corner_radius: CornerRadius,

    /// Colour of the tab's background. By `Default` it's [`Color32::WHITE`].
    pub bg_fill: Color32,

    /// Color of the title text.
    pub text_color: Color32,
}

/// Specifies the look and feel of the tab body.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct TabBodyStyle {
    /// Inner margin of tab body. By `Default` it's `Margin::same(4.0)`.
    pub inner_margin: Margin,

    /// The stroke of the tabs border. By `Default` it's ['Stroke::default'].
    pub stroke: Stroke,

    /// Tab corner radius. By `Default` it's [`CornerRadius::default`].
    pub corner_radius: CornerRadius,

    /// Colour of the tab's background. By `Default` it's [`Color32::WHITE`].
    pub bg_fill: Color32,
}

/// Specifies the look and feel of the tab drop overlay.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct OverlayStyle {
    /// Sets selection color for the placing area of the tab where this tab targeted on it.
    /// By `Default` it's `(0, 191, 255)` (light blue) with `0.5` capacity.
    pub selection_color: Color32,

    /// Width of stroke when a selection uses an outline instead of filled rectangle.
    pub selection_stroke_width: f32,

    /// Units of padding between each button.
    pub button_spacing: f32,

    /// Max side length of a button on the overlay.
    pub max_button_size: f32,

    /// Style of the additional highlighting rectangle drawn on the surface which you're attempting to drop a tab in.
    ///
    /// By default this value shows no highlighting.
    pub hovered_leaf_highlight: LeafHighlighting,

    /// Opacity which surfaces will fade to in a range of `0.0..=1.0`.
    pub surface_fade_opacity: f32,

    /// The color of the overlay buttons.
    pub button_color: Color32,

    /// The stroke of the button border.
    pub button_border_stroke: Stroke,

    /// The type of overlay used.
    pub overlay_type: OverlayType,

    /// The feel of the overlay, timings, detection, etc.
    pub feel: OverlayFeel,
}

/// Specifies the feel of the tab drop overlay, i.e anything non visual about the overlay.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct OverlayFeel {
    /// range is `0.0..=1.0`.
    pub window_drop_coverage: f32,

    /// range is `0.0..=1.0`.
    pub center_drop_coverage: f32,

    /// The amount of time windows should stay faded despite not needing to, prevents quick mouse movements from causing flashing.
    pub fade_hold_time: f32,

    /// Amount of time the overlay waits before dropping a preference it may have for a node.
    pub max_preference_time: f32,

    /// Units which the buttons interact area will be expanded by.
    pub interact_expansion: f32,
}

/// Specifies the type of overlay used.
#[derive(Copy, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum OverlayType {
    /// Shows highlighted areas predicting where a dropped tab would land were it to be dropped this frame.
    ///
    /// Always used when hovering over tabs and tab head.
    HighlightedAreas,

    /// Shows icons indicating the possible drop positions which the user may hover over to drop a tab at that given location.
    ///
    /// This is the default type of overlay for leaves.
    Widgets,
}

/// Highlighting on the currently hovered leaf.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct LeafHighlighting {
    /// Fill color.
    pub color: Color32,

    /// Rounding of the resulting rectangle.
    pub corner_radius: CornerRadius,

    /// Stroke.
    pub stroke: Stroke,

    /// Amount of egui units which each side should expand.
    pub expansion: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            dock_area_padding: None,
            main_surface_border_stroke: Stroke::new(f32::default(), Color32::BLACK),
            main_surface_border_rounding: CornerRadius::default(),
            buttons: ButtonsStyle::default(),
            separator: SeparatorStyle::default(),
            cross_split_toggle: CrossSplitToggleStyle::default(),
            tab_bar: TabBarStyle::default(),
            tab: TabStyle::default(),
            overlay: OverlayStyle::default(),
        }
    }
}

impl Default for ButtonsStyle {
    fn default() -> Self {
        Self {
            close_tab_color: Color32::WHITE,
            close_tab_active_color: Color32::WHITE,
            close_tab_bg_fill: Color32::GRAY,

            add_tab_align: TabAddAlign::Right,
            add_tab_color: Color32::WHITE,
            add_tab_active_color: Color32::WHITE,
            add_tab_bg_fill: Color32::GRAY,
            add_tab_border_color: Color32::BLACK,

            close_all_tabs_color: Color32::WHITE,
            close_all_tabs_active_color: Color32::WHITE,
            close_all_tabs_bg_fill: Color32::GRAY,
            close_all_tabs_border_color: Color32::BLACK,
            close_all_tabs_disabled_color: Color32::LIGHT_GRAY,

            collapse_tabs_color: Color32::WHITE,
            collapse_tabs_active_color: Color32::WHITE,
            collapse_tabs_bg_fill: Color32::GRAY,
            collapse_tabs_border_color: Color32::BLACK,

            minimize_window_color: Color32::WHITE,
            minimize_window_active_color: Color32::WHITE,
            minimize_window_bg_fill: Color32::GRAY,
            minimize_window_border_color: Color32::BLACK,
        }
    }
}

impl Default for SeparatorStyle {
    fn default() -> Self {
        Self {
            width: 1.0,
            extra_interact_width: 2.0,
            extra: 175.0,
            color_idle: Color32::BLACK,
            color_hovered: Color32::GRAY,
            color_dragged: Color32::WHITE,
        }
    }
}

impl Default for CrossSplitToggleStyle {
    fn default() -> Self {
        Self {
            size: 10.0,
            catch_extra: 1.0,
            hold_extra: 0.5,
            align_tolerance: 8.0,
            icon_color: Color32::from_gray(27),
        }
    }
}

impl Default for TabBarStyle {
    fn default() -> Self {
        Self {
            bg_fill: Color32::WHITE,
            height: 24.0,
            inner_margin: Margin::ZERO,
            show_scroll_bar_on_overflow: true,
            corner_radius: CornerRadius::default(),
            hline_color: Color32::BLACK,
            fill_tab_bar: false,
        }
    }
}

impl Default for TabStyle {
    fn default() -> Self {
        Self {
            active: TabInteractionStyle::default(),
            inactive: TabInteractionStyle {
                text_color: Color32::DARK_GRAY,
                ..Default::default()
            },
            focused: TabInteractionStyle {
                text_color: Color32::BLACK,
                ..Default::default()
            },
            hovered: TabInteractionStyle {
                text_color: Color32::BLACK,
                ..Default::default()
            },
            active_with_kb_focus: TabInteractionStyle::default(),
            inactive_with_kb_focus: TabInteractionStyle {
                text_color: Color32::DARK_GRAY,
                ..Default::default()
            },
            focused_with_kb_focus: TabInteractionStyle {
                text_color: Color32::BLACK,
                ..Default::default()
            },
            spacing: 0.0,
            tab_body: TabBodyStyle::default(),
            hline_below_active_tab_name: false,
            minimum_width: None,
        }
    }
}

impl Default for TabInteractionStyle {
    fn default() -> Self {
        Self {
            bg_fill: Color32::WHITE,
            outline_color: Color32::BLACK,
            corner_radius: CornerRadius::default(),
            text_color: Color32::DARK_GRAY,
        }
    }
}

impl Default for TabBodyStyle {
    fn default() -> Self {
        Self {
            inner_margin: Margin::same(4),
            stroke: Stroke::default(),
            corner_radius: CornerRadius::default(),
            bg_fill: Color32::WHITE,
        }
    }
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            selection_color: Color32::from_rgb(0, 191, 255).linear_multiply(0.5),
            selection_stroke_width: 1.0,
            button_spacing: 10.0,
            max_button_size: 100.0,

            surface_fade_opacity: 0.1,

            hovered_leaf_highlight: Default::default(),
            button_color: Color32::from_gray(140),
            button_border_stroke: Stroke::new(1.0_f32, Color32::from_gray(60)),
            overlay_type: OverlayType::Widgets,
            feel: Default::default(),
        }
    }
}

impl Default for OverlayFeel {
    fn default() -> Self {
        Self {
            max_preference_time: 0.3,
            window_drop_coverage: 0.5,
            center_drop_coverage: 0.25,
            fade_hold_time: 0.2,
            interact_expansion: 20.0,
        }
    }
}

impl Default for LeafHighlighting {
    fn default() -> Self {
        Self {
            color: Color32::TRANSPARENT,
            corner_radius: CornerRadius::same(0),
            stroke: Stroke::NONE,
            expansion: 0.0,
        }
    }
}

impl Style {
    pub(crate) const TAB_ADD_BUTTON_SIZE: f32 = 24.0;
    pub(crate) const TAB_ADD_PLUS_SIZE: f32 = 12.0;
    pub(crate) const TAB_CLOSE_BUTTON_SIZE: f32 = 24.0;
    pub(crate) const TAB_CLOSE_X_SIZE: f32 = 9.0;
    pub(crate) const TAB_CLOSE_ALL_BUTTON_SIZE: f32 = 24.0;
    pub(crate) const TAB_CLOSE_ALL_SIZE: f32 = 10.0;
    pub(crate) const TAB_COLLAPSE_BUTTON_SIZE: f32 = 24.0;
    pub(crate) const TAB_COLLAPSE_ARROW_SIZE: f32 = 10.0;
    pub(crate) const TAB_EXPAND_BUTTON_SIZE: f32 = 24.0;
    pub(crate) const TAB_EXPAND_ARROW_SIZE: f32 = 10.0;
}

impl Style {
    /// Derives relevant fields from `egui::Style` and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`Style::main_surface_border_stroke`]
    ///
    /// See also: [`ButtonsStyle::from_egui`], [`SeparatorStyle::from_egui`], [`TabBarStyle::from_egui`],
    /// [`TabStyle::from_egui`]
    pub fn from_egui(style: &egui::Style) -> Self {
        Self {
            main_surface_border_stroke: Stroke::NONE,
            main_surface_border_rounding: CornerRadius::ZERO,
            buttons: ButtonsStyle::from_egui(style),
            separator: SeparatorStyle::from_egui(style),
            tab_bar: TabBarStyle::from_egui(style),
            tab: TabStyle::from_egui(style),
            overlay: OverlayStyle::from_egui(style),
            cross_split_toggle: CrossSplitToggleStyle::from_egui(style),
            ..Self::default()
        }
    }
}

impl ButtonsStyle {
    /// Derives relevant fields from `egui::Style` and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`ButtonsStyle::close_tab_bg_fill`]
    /// - [`ButtonsStyle::close_tab_color`]
    /// - [`ButtonsStyle::close_tab_active_color`]
    /// - [`ButtonsStyle::add_tab_bg_fill`]
    /// - [`ButtonsStyle::add_tab_color`]
    /// - [`ButtonsStyle::add_tab_active_color`]
    /// - [`ButtonsStyle::add_tab_border_color`]
    /// - [`ButtonsStyle::close_all_tabs_bg_fill`]
    /// - [`ButtonsStyle::close_all_tabs_color`]
    /// - [`ButtonsStyle::close_all_tabs_active_color`]
    /// - [`ButtonsStyle::close_all_tabs_border_color`]
    /// - [`ButtonsStyle::collapse_tabs_bg_fill`]
    /// - [`ButtonsStyle::collapse_tabs_color`]
    /// - [`ButtonsStyle::collapse_tabs_active_color`]
    /// - [`ButtonsStyle::collapse_tabs_border_color`]
    pub fn from_egui(style: &egui::Style) -> Self {
        Self {
            close_tab_bg_fill: style.visuals.widgets.hovered.bg_fill,
            close_tab_color: style.visuals.text_color(),
            close_tab_active_color: style.visuals.strong_text_color(),
            add_tab_bg_fill: style.visuals.widgets.hovered.bg_fill,
            add_tab_color: style.visuals.text_color(),
            add_tab_active_color: style.visuals.strong_text_color(),
            add_tab_border_color: style.visuals.widgets.noninteractive.bg_fill,
            close_all_tabs_bg_fill: style.visuals.widgets.hovered.bg_fill,
            close_all_tabs_color: style.visuals.text_color(),
            close_all_tabs_active_color: style.visuals.strong_text_color(),
            close_all_tabs_border_color: style.visuals.widgets.noninteractive.bg_fill,
            close_all_tabs_disabled_color: style.visuals.widgets.inactive.bg_fill,
            collapse_tabs_bg_fill: style.visuals.widgets.hovered.bg_fill,
            collapse_tabs_color: style.visuals.text_color(),
            collapse_tabs_active_color: style.visuals.strong_text_color(),
            collapse_tabs_border_color: style.visuals.widgets.noninteractive.bg_fill,
            minimize_window_bg_fill: style.visuals.widgets.hovered.bg_fill,
            minimize_window_color: style.visuals.text_color(),
            minimize_window_active_color: style.visuals.strong_text_color(),
            minimize_window_border_color: style.visuals.widgets.noninteractive.bg_fill,
            ..ButtonsStyle::default()
        }
    }
}

impl SeparatorStyle {
    /// Derives relevant fields from `egui::Style` and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`SeparatorStyle::color_idle`]
    /// - [`SeparatorStyle::color_hovered`]
    /// - [`SeparatorStyle::color_dragged`]
    pub fn from_egui(style: &egui::Style) -> Self {
        Self {
            // Same as egui panel resize colors:
            color_idle: style.visuals.widgets.noninteractive.bg_stroke.color, // dim
            color_hovered: style.visuals.widgets.hovered.fg_stroke.color,     // bright
            color_dragged: style.visuals.widgets.active.fg_stroke.color,      // bright
            ..SeparatorStyle::default()
        }
    }
}

impl TabBarStyle {
    /// Derives relevant fields from `egui::Style` and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`TabBarStyle::bg_fill`]
    /// - [`TabBarStyle::hline_color`]
    pub fn from_egui(style: &egui::Style) -> Self {
        Self {
            bg_fill: style.visuals.extreme_bg_color,
            corner_radius: CornerRadius {
                nw: style.visuals.widgets.inactive.corner_radius.nw + 2,
                ne: style.visuals.widgets.inactive.corner_radius.ne + 2,
                sw: 0,
                se: 0,
            },
            hline_color: style.visuals.widgets.noninteractive.bg_stroke.color,
            ..TabBarStyle::default()
        }
    }
}

impl TabStyle {
    /// Derives tab styles from `egui::Style`.
    ///
    /// See also: [`TabInteractionStyle::from_egui_active`], [`TabInteractionStyle::from_egui_inactive`],
    /// [`TabInteractionStyle::from_egui_focused`], [`TabInteractionStyle::from_egui_hovered`], [`TabBodyStyle::from_egui`],
    pub fn from_egui(style: &egui::Style) -> TabStyle {
        Self {
            active: TabInteractionStyle::from_egui_active(style),
            inactive: TabInteractionStyle::from_egui_inactive(style),
            focused: TabInteractionStyle::from_egui_focused(style),
            hovered: TabInteractionStyle::from_egui_hovered(style),
            active_with_kb_focus: TabInteractionStyle::from_egui_active_with_kb_focus(style),
            inactive_with_kb_focus: TabInteractionStyle::from_egui_inactive_with_kb_focus(style),
            focused_with_kb_focus: TabInteractionStyle::from_egui_focused_with_kb_focus(style),
            tab_body: TabBodyStyle::from_egui(style),
            ..Default::default()
        }
    }
}

impl TabInteractionStyle {
    /// Derives relevant fields from `egui::Style` for an active tab and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`TabInteractionStyle::outline_color`]
    /// - [`TabInteractionStyle::bg_fill`]
    /// - [`TabInteractionStyle::text_color`]
    pub fn from_egui_active(style: &egui::Style) -> Self {
        Self {
            outline_color: style.visuals.widgets.noninteractive.bg_stroke.color,
            bg_fill: style.visuals.window_fill(),
            text_color: style.visuals.text_color(),
            corner_radius: CornerRadius {
                sw: 0,
                se: 0,
                ..style.visuals.widgets.active.corner_radius
            },
        }
    }

    /// Derives relevant fields from `egui::Style` for an inactive tab and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`TabInteractionStyle::outline_color`]
    /// - [`TabInteractionStyle::bg_fill`]
    /// - [`TabInteractionStyle::text_color`]
    pub fn from_egui_inactive(style: &egui::Style) -> Self {
        Self {
            text_color: style.visuals.text_color(),
            bg_fill: tint_color_towards(style.visuals.window_fill, style.visuals.extreme_bg_color),
            outline_color: tint_color_towards(
                style.visuals.widgets.noninteractive.bg_stroke.color,
                style.visuals.extreme_bg_color,
            ),
            ..TabInteractionStyle::from_egui_active(style)
        }
    }

    /// Derives relevant fields from `egui::Style` for a focused tab and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`TabInteractionStyle::outline_color`]
    /// - [`TabInteractionStyle::bg_fill`]
    /// - [`TabInteractionStyle::text_color`]
    pub fn from_egui_focused(style: &egui::Style) -> Self {
        Self {
            text_color: style.visuals.strong_text_color(),
            ..TabInteractionStyle::from_egui_active(style)
        }
    }

    /// Derives relevant fields from `egui::Style` for a hovered tab and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`TabInteractionStyle::outline_color`]
    /// - [`TabInteractionStyle::bg_fill`]
    /// - [`TabInteractionStyle::text_color`]
    pub fn from_egui_hovered(style: &egui::Style) -> Self {
        Self {
            text_color: style.visuals.strong_text_color(),
            outline_color: style.visuals.widgets.hovered.bg_stroke.color,
            ..TabInteractionStyle::from_egui_inactive(style)
        }
    }

    /// Derives relevant fields from `egui::Style` for an active tab with keyboard focus and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`TabInteractionStyle::outline_color`]
    /// - [`TabInteractionStyle::bg_fill`]
    /// - [`TabInteractionStyle::text_color`]
    pub fn from_egui_active_with_kb_focus(style: &egui::Style) -> Self {
        Self {
            text_color: style.visuals.strong_text_color(),
            outline_color: style.visuals.widgets.hovered.bg_stroke.color,
            ..TabInteractionStyle::from_egui_active(style)
        }
    }

    /// Derives relevant fields from `egui::Style` for an inactive tab with keyboard focus and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`TabInteractionStyle::outline_color`]
    /// - [`TabInteractionStyle::bg_fill`]
    /// - [`TabInteractionStyle::text_color`]
    pub fn from_egui_inactive_with_kb_focus(style: &egui::Style) -> Self {
        Self {
            text_color: style.visuals.strong_text_color(),
            outline_color: style.visuals.widgets.hovered.bg_stroke.color,
            ..TabInteractionStyle::from_egui_inactive(style)
        }
    }

    /// Derives relevant fields from `egui::Style` for a focused tab with keyboard focus and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`TabInteractionStyle::outline_color`]
    /// - [`TabInteractionStyle::bg_fill`]
    /// - [`TabInteractionStyle::text_color`]
    pub fn from_egui_focused_with_kb_focus(style: &egui::Style) -> Self {
        Self {
            text_color: style.visuals.strong_text_color(),
            outline_color: style.visuals.widgets.hovered.bg_stroke.color,
            ..TabInteractionStyle::from_egui_focused(style)
        }
    }
}

impl TabBodyStyle {
    /// Derives relevant fields from `egui::Style` and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`TabBodyStyle::inner_margin`]
    /// - [`TabBodyStyle::stroke]
    /// - [`TabBodyStyle::bg_fill`]
    pub fn from_egui(style: &egui::Style) -> Self {
        Self {
            inner_margin: style.spacing.window_margin,
            stroke: style.visuals.widgets.noninteractive.bg_stroke,
            corner_radius: style.visuals.widgets.active.corner_radius,
            bg_fill: style.visuals.window_fill(),
        }
    }
}

impl OverlayStyle {
    /// Derives relevant fields from `egui::Style` and sets the remaining fields to their default values.
    ///
    /// Fields overwritten by [`egui::Style`] are:
    /// - [`OverlayStyle::selection_color`]
    /// - [`OverlayStyle::button_spacing]
    /// - [`OverlayStyle::button_color`]
    /// - [`OverlayStyle::button_border_stroke`]
    pub fn from_egui(style: &egui::Style) -> Self {
        Self {
            selection_color: style.visuals.selection.bg_fill.linear_multiply(0.5),
            button_spacing: style.spacing.icon_spacing,
            button_color: style.visuals.widgets.noninteractive.fg_stroke.color,
            button_border_stroke: style.visuals.widgets.noninteractive.bg_stroke,
            ..Default::default()
        }
    }
}
