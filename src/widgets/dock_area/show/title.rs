//! A tab's title: measured once, then drawn — across a tab bar or down a side strip.
//!
//! A title is a row of [`Atoms`] the consumer hands over — a name, an icon, or both — and the
//! same row is drawn in two places that run along different axes. So everything here is written
//! in terms of *along* and *across*, and which of those is x and which is y is the caller's
//! `vertical`.
//!
//! Measuring needs a `Ui` (a galley is laid out against a font and a style) and so does drawing,
//! which is why none of this went to `core` with the arithmetic that shares the room out. What
//! it is instead is one place: `leaf.rs` measures a title, asks [`crate::core::fit`] how much of
//! the row it gets, and hands both back here to be painted.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use egui::{
    Align2, AtomKind, Atoms, Color32, Galley, Mesh, Painter, Rect, Response, Shape, SizedAtom,
    SizedAtomKind, TextStyle, TextWrapMode, Ui, UiBuilder, epaint::TextShape, pos2, vec2,
};

use crate::core::fit::STRIP_NAME_PADDING;
use crate::utils::clip_to;

/// How far a name fades out where it runs past the room it was given.
///
/// An ellipsis is the other way of saying "this was cut", and it is what this crate used to draw.
/// It costs about ten pixels of the name to say so, which is a third of what a squeezed tab has
/// left. Chrome has faded rather than clipped since 32, Firefox since 53 (bug 658467 — "the
/// fadeout gives 1-2 more characters to the user and looks smoother"), VS Code since #39829.
pub(super) const FADE_LENGTH: f32 = 8.0;

/// A title measured into its atoms: what each one takes, and what stands between them.
///
/// A title is a row of [`Atoms`] — a name, an icon, or both — and the same row is drawn in two
/// places that run along different axes: a tab bar across the screen and a side strip down it.
/// So everything here is written in terms of *along* and *across*, and which axis is which is the
/// caller's `vertical`.
pub(super) struct SizedTitle {
    /// The atoms in the order they are drawn: the first sits at the start of the slot, which is
    /// the left of a bar and the bottom of a strip (the end a turned name is read from).
    atoms: Vec<SizedAtom<'static>>,

    /// What stands between two atoms, along the axis.
    gap: f32,
}

impl SizedTitle {
    /// The room the whole title wants along the axis, gaps included.
    pub(super) fn length(&self, vertical: bool) -> f32 {
        let atoms: f32 = self
            .atoms
            .iter()
            .map(|atom| atom_length(atom, vertical))
            .sum();
        atoms + self.gap * self.atoms.len().saturating_sub(1) as f32
    }
}

/// The room one atom takes *along* the axis.
///
/// Text turns with the strip — a name in a side strip reads bottom to top — so what it takes
/// along the axis is its width either way. Nothing else turns: an icon stays upright, which is
/// what makes it legible in a strip at all, so it takes its height along a vertical axis.
fn atom_length(atom: &SizedAtom<'_>, vertical: bool) -> f32 {
    match atom.kind {
        SizedAtomKind::Text(_) => atom.size.x,
        _ if vertical => atom.size.y,
        _ => atom.size.x,
    }
}

/// The room one atom takes *across* the axis — the other half of [`atom_length`], same rule.
fn atom_breadth(atom: &SizedAtom<'_>, vertical: bool) -> f32 {
    match atom.kind {
        SizedAtomKind::Text(_) => atom.size.y,
        _ if vertical => atom.size.x,
        _ => atom.size.y,
    }
}

/// Lays a title out in full, however long it is.
///
/// Nothing is cut at layout time: what does not fit is clipped to the slot and faded into the
/// background there, so the galley has to carry every glyph the name has. How much of it will be
/// *seen* is [`crate::core::fit::fit_strip_names`]'s or [`crate::core::fit::fit_tab_widths`]'s
/// answer, not the text layout's.
///
/// An image is capped at the **type size** of the text beside it — the em, not the line box — so
/// that an icon reads as one of the letters rather than as something standing behind them. It is a
/// ceiling and not a resize: a smaller icon stays smaller, and an atom given a size of its own
/// ([`egui::AtomExt::atom_size`]) keeps it.
///
/// # Why a ceiling and not an offer of room
///
/// Only one of egui's three [`egui::ImageFit`]s asks how much room it has. `Fraction` — the
/// default for an image loaded from bytes or a uri — takes what it is offered; `Exact` and
/// `Original` do not, and `Original` is exactly what an icon *registry* hands over (rerun's
/// `Icon::as_image` is `fit_to_original_size`, and so is every set that follows it). An icon set
/// drawn on a 24 px grid therefore landed 24 px tall no matter what room the title offered it —
/// which, in a 24 px tab bar, is the whole tab from edge to edge. `max_size` is the one lever all
/// three fits obey.
///
/// Sharpness is not this function's business, tempting as it is to make it so: [`egui::Image`]
/// reloads at exactly the rectangle it paints ("important for getting crisp SVG:s", says the
/// comment in `paint_at`), so a vector icon is rasterised at the size it lands at whatever size
/// hint the measuring offered. A 24 px drawing at 14 px looks soft because a 2 px stroke becomes a
/// 1.2 px one, and no amount of arranging here changes that.
///
/// # Why the em and not the line height, which is what a button does
///
/// [`egui::Button`] limits an icon to `atom_max_height_font_size`, and that helper — the name
/// notwithstanding — hands back the *row height*. It is the right answer there and the wrong one
/// here: a button grows to fit its contents, while **a tab bar is a fixed
/// [`crate::TabBarStyle::height`]**. Measured at the typography this crate is used with — a 14 pt
/// button face, a 16.4 pt line, a 24 pt bar — the line box leaves 3.8 pt of tab above and below
/// the icon against the 7 the letters leave, so even a correctly *offered* icon still reads as
/// crowding its tab. The em is the size the letters themselves were asked for, which puts the two
/// on the same optical footing whatever the bar is styled to.
///
/// Only pictures are capped. Text is laid out against the width alone (the height of the room it
/// is offered does not enter into a galley), and a nested [`egui::AtomLayout`] is a widget of its
/// own that was never promised a particular height.
pub(super) fn measure_title(ui: &Ui, title: Atoms<'static>) -> SizedTitle {
    let line = ui.text_style_height(&TextStyle::Button);
    let em = TextStyle::Button.resolve(ui.style()).size;
    SizedTitle {
        atoms: title
            .into_iter()
            .map(|mut atom| {
                // An atom carrying its own size is answering this question itself, so it is left
                // alone — a ceiling here would silently overrule it, and `atom_size` is the
                // documented way to say "this icon is different".
                if atom.size.is_none()
                    && matches!(atom.kind, AtomKind::Image(_))
                    && let AtomKind::Image(image) =
                        std::mem::replace(&mut atom.kind, AtomKind::Empty)
                {
                    // The ceiling goes on the *image* rather than on the atom, because
                    // `Atom::max_size` only bounds the room offered, and it is exactly the fits
                    // that ignore the offer (`Original`, `Exact`) that need holding down. It is
                    // also the whole of the arrangement: a `Fraction` image is capped by the same
                    // `max_size` on its way through `calc_size`, so offering it the em on top of
                    // that would be the same number said twice.
                    atom.kind = AtomKind::Image(image.max_height(em));
                }
                atom.into_sized(
                    ui,
                    vec2(f32::INFINITY, line),
                    Some(TextWrapMode::Extend),
                    TextStyle::Button.into(),
                )
            })
            .collect(),
        gap: ui.spacing().icon_spacing,
    }
}

/// Fades whatever is under `rect` into `into` — clear at the start of the rectangle, solid at its
/// end — where "end" is the end of the strip's own direction.
///
/// This is how a name says it was cut. egui has no text mask, so the fade is painted *over* the
/// glyphs in the colour behind them; that is why it takes the background it is fading into rather
/// than working it out, and why a translucent tab background fades approximately rather than
/// exactly.
pub(super) fn fade_out(ui: &Ui, rect: Rect, into: Color32, vertical: bool) {
    // Premultiplied alpha, so a transparent vertex is transparent *black* and the interpolation
    // between it and an opaque colour is a clean ramp rather than a walk through grey.
    let (clear, solid) = if vertical {
        // A strip's text is turned a quarter turn anticlockwise: it reads bottom to top, so the
        // end of the name is at the top of the slot.
        (
            [rect.left_bottom(), rect.right_bottom()],
            [rect.left_top(), rect.right_top()],
        )
    } else {
        (
            [rect.left_top(), rect.left_bottom()],
            [rect.right_top(), rect.right_bottom()],
        )
    };

    let mut mesh = Mesh::default();
    mesh.colored_vertex(clear[0], Color32::TRANSPARENT);
    mesh.colored_vertex(clear[1], Color32::TRANSPARENT);
    mesh.colored_vertex(solid[0], into);
    mesh.colored_vertex(solid[1], into);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(2, 1, 3);
    ui.painter().add(Shape::mesh(mesh));
}

/// The room a laid-out name takes along the strip: the title itself, plus padding at each end.
pub(super) fn strip_length(title: &SizedTitle, vertical: bool) -> f32 {
    title.length(vertical) + 2.0 * STRIP_NAME_PADDING
}

/// The rectangle a run of `length` along the strip takes, starting `cursor` along it.
///
/// A strip runs down the screen and a bar runs across it: everything a strip draws is the full
/// width of one and the full height of the other, so only which axis is which differs.
pub(super) fn strip_slot(rect: Rect, vertical: bool, cursor: f32, length: f32) -> Rect {
    if vertical {
        Rect::from_min_size(pos2(rect.left(), cursor), vec2(rect.width(), length))
    } else {
        Rect::from_min_size(pos2(cursor, rect.top()), vec2(length, rect.height()))
    }
}

/// Draws `galley` centred in `cell`, turned a quarter turn when the strip is vertical.
///
/// Anchored at the middle of the galley, so the turn happens about the text's own centre.
/// Anticlockwise (`angle` counts clockwise), which is what makes the glyphs run bottom-to-top —
/// the direction a side bar is read in, and therefore the direction a name that does not fit runs
/// *out* of: its first letter is at the bottom of the slot and its last is past the top.
fn paint_galley(
    painter: &Painter,
    cell: Rect,
    galley: Arc<Galley>,
    color: Color32,
    vertical: bool,
) {
    let mut text = TextShape::new(cell.center() - galley.size() / 2.0, galley, color);
    if vertical {
        text = text.with_angle_and_anchor(-FRAC_PI_2, Align2::CENTER_CENTER);
    }
    painter.add(text);
}

/// Draws `title` in `slot`, fading out into `background` where it runs past the room it was given.
///
/// The atoms are laid along the axis in the order the consumer gave them, each one centred across
/// it. `interact` is what a nested [`egui::AtomLayout`] is painted against — an atom that is a
/// widget in its own right needs a [`Response`] to interact with, and every title the consumer
/// gives has one (the tab, or the name in a strip). A title the dock makes up itself has no such
/// atom and passes `None`.
pub(super) fn paint_title(
    ui: &mut Ui,
    slot: Rect,
    title: SizedTitle,
    color: Color32,
    background: Color32,
    vertical: bool,
    interact: Option<&Response>,
) {
    let length = title.length(vertical);
    let room = if vertical {
        slot.height()
    } else {
        slot.width()
    };
    // A whole pixel of slack: a name that was given exactly its own length comes back a hair
    // short of it once the slot has been snapped to the pixel grid, and fading the last half
    // pixel of a name that fits would be a smudge with nothing behind it.
    let cut = length > room + 1.0;

    // A title that fits sits in the middle of its slot. One that does not starts at the slot's
    // beginning instead and runs out of the far end — centring it would hide the first atoms as
    // well as the last, and the first are the half that tells the panels apart. It is also what
    // keeps an icon: put first, it is the last thing a squeeze takes away, which is what a
    // browser's favicon does with the room a squeezed tab has left.
    let mut cursor = match (cut, vertical) {
        // A strip reads bottom to top, so its cursor starts at the bottom and walks *up*.
        (false, true) => slot.center().y + length / 2.0,
        (true, true) => slot.bottom(),
        (false, false) => slot.center().x - length / 2.0,
        (true, false) => slot.left(),
    };

    // Clipped to the slot, and never wider than what this `Ui` was already clipped to — the
    // bar's own edge has to keep cutting the last tab off (see `nothing_widens_its_clip`).
    let inner = &mut ui.new_child(UiBuilder::new().max_rect(slot));
    clip_to(inner, slot);

    let gap = title.gap;
    for atom in title.atoms {
        let run = atom_length(&atom, vertical);
        let breadth = atom_breadth(&atom, vertical);
        let cell = if vertical {
            Rect::from_min_size(
                pos2(slot.center().x - breadth / 2.0, cursor - run),
                vec2(breadth, run),
            )
        } else {
            Rect::from_min_size(
                pos2(cursor, slot.center().y - breadth / 2.0),
                vec2(run, breadth),
            )
        };
        cursor += if vertical { -(run + gap) } else { run + gap };

        match atom.kind {
            SizedAtomKind::Text(galley) => {
                paint_galley(inner.painter(), cell, galley, color, vertical);
            }
            SizedAtomKind::Image { image, size: _ } => {
                // An icon takes the colour the *name* beside it was given, so a monochrome icon
                // follows the tab through active/inactive/hovered and through a theme change the
                // same way its text does. Without this it would be painted in whatever the image
                // itself is, and a set drawn for a dark theme would vanish on a light one.
                //
                // Only a *default* tint is replaced: a consumer that asked for a colour asked for
                // that colour — a multi-coloured logo, or an icon deliberately off the text's hue —
                // and this must not silently repaint it. `Color32::WHITE` is `ImageOptions`' own
                // default and is the identity of the multiply, so "untinted" and "tinted white"
                // are the same request and are answered the same way.
                let image = if image.image_options().tint == Color32::WHITE {
                    image.tint(color)
                } else {
                    image
                };
                image.paint_at(inner, cell);
            }
            SizedAtomKind::Empty { .. } => {}
            SizedAtomKind::Layout(layout) => {
                // Painted only where there is a `Response` to paint it against; see the doc above.
                debug_assert!(
                    interact.is_some(),
                    "a title with a nested AtomLayout has to be drawn against a Response"
                );
                if let Some(response) = interact {
                    layout.paint_at(inner, cell, response.clone());
                }
            }
        }
    }

    if cut {
        // Where the title runs out of room it fades into the background rather than stopping
        // dead; that fade *is* the statement "there is more of this than is written", the one an
        // ellipsis used to make at the cost of a third of what a squeezed tab has left.
        let fade = if vertical {
            Rect::from_min_max(slot.min, pos2(slot.right(), slot.top() + FADE_LENGTH))
        } else {
            Rect::from_min_max(pos2(slot.right() - FADE_LENGTH, slot.top()), slot.max)
        };
        fade_out(ui, fade, background, vertical);
    }
}
