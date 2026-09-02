//! The little painters: the arrow a collapsed leaf wears, the doubled one that stows a side, the
//! chevrons on a window's own buttons, the mark that closes one, and the arrows on a junction
//! handle.
//!
//! They were seven functions in three files and no two of them agreed on what they took: a
//! `&mut Ui` where a `&Painter` would do, a whole `&Style` where one colour would, a `&mut
//! Response` that was only ever read. Here each takes what it draws with and then what it draws
//! into, and the argument that used to be in the name — which way it points, which side it is on,
//! whether the leaf is collapsed — is a [`Dir`].
//!
//! Which is what the gathering was for: three of the four arrows turn out to be one triangle seen
//! from three angles, and both chevrons turn out to be that same triangle three times over.

use std::ops::RangeInclusive;

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, pos2, vec2};

/// The three corners of the triangle [`triangle`] fills, in the order it lists them.
///
/// Separate from the painting so that the shapes the chevrons are built out of can be judged
/// without a `Ui` — which is the whole reason the rewrite below is allowed to be a rewrite.
fn corners(rect: Rect, dir: Dir) -> [Pos2; 3] {
    match dir {
        Dir::Right => [rect.left_top(), rect.right_center(), rect.left_bottom()],
        Dir::Down => [rect.left_top(), rect.right_top(), rect.center_bottom()],
        Dir::Left => [rect.right_top(), rect.left_center(), rect.right_bottom()],
    }
}

/// Which way a glyph points.
///
/// Up is missing because nothing points that way: a strip folds towards the edge it is pressed
/// against, and a leaf opens downwards or sideways.
#[derive(Clone, Copy, Debug)]
pub(super) enum Dir {
    Right,
    Down,
    Left,
}

/// A filled triangle in `rect`: its base along the side `dir` comes from, its apex in the middle
/// of the side `dir` points at.
///
/// The vertices are listed per direction rather than derived from one another, because the fill
/// is feathered along the outward normal and that normal follows the winding — two orders that
/// name the same triangle do not paint the same pixels at its edge.
pub(super) fn triangle(painter: &Painter, rect: Rect, dir: Dir, color: Color32) {
    painter.add(Shape::convex_polygon(
        corners(rect, dir).to_vec(),
        color,
        Stroke::NONE,
    ));
}

/// The arrow of "put my whole side away": the ordinary collapse triangle, doubled.
///
/// Doubled rather than a glyph of its own, because the gesture is not a different action — it is
/// the same fold one level up, and the icon says so: what the plain arrow does to this leaf, this
/// one does to everything beside it.
pub(super) fn stow_arrow(painter: &Painter, rect: Rect, color: Color32) {
    // Two triangles stacked along the arrow's own axis, each half as tall, with a pixel
    // between them so they read as two at the size this is drawn.
    let half = rect.height() * 0.5;
    for step in 0..2 {
        let top = rect.top() + half * step as f32;
        let cell = Rect::from_min_max(
            pos2(rect.left(), top),
            pos2(rect.right(), top + half - 1.0),
        );
        triangle(painter, cell, Dir::Down, color);
    }
}

/// A window button's chevron: a head, the same shape again behind it, and a notch cut out of the
/// second one in the colour behind the button.
///
/// The two halves are the same triangle drawn in the two halves of `rect`, one after the other
/// along `dir`; the notch is a third, a quarter of the way into the far half and half as wide.
/// Written that way rather than as two sets of corner points because that is what it *is* — the
/// down-pointing and right-pointing chevrons were the same three triangles under two spellings.
pub(super) fn chevron(painter: &Painter, rect: Rect, dir: Dir, color: Color32, notch: Color32) {
    triangle(painter, part(rect, dir, 0.0..=0.5, 0.0..=1.0), dir, color);
    triangle(painter, part(rect, dir, 0.5..=1.0, 0.0..=1.0), dir, color);
    triangle(painter, part(rect, dir, 0.5..=0.75, 0.25..=0.75), dir, notch);
}

/// The mark on a window's close-everything button: a sheet with its corner turned, and a cross.
pub(super) fn close_window(painter: &Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.0_f32, color);
    painter.add(Shape::line(
        vec![
            rect.right_center().lerp(rect.right_bottom(), 0.5),
            rect.right_bottom(),
            rect.left_bottom(),
            rect.left_top(),
            rect.center_top().lerp(rect.left_top(), 0.5),
        ],
        stroke,
    ));
    painter.line_segment([rect.center_top(), rect.right_center()], stroke);
    painter.line_segment([rect.center(), rect.right_top()], stroke);
}

/// One arrow of a handle's icon: a stem from near the centre outwards, and two barbs at its tip.
///
/// The odd one out, and deliberately so: a handle's arrows radiate from a point at whatever angle
/// the junction's arms leave, so this one is placed by a centre and a direction rather than by a
/// rectangle, and it is drawn with a stroke rather than filled.
pub(super) fn barbed_arrow(painter: &Painter, center: Pos2, dir: Vec2, arm: f32, stroke: Stroke) {
    let tip = center + dir * arm;
    let base = center + dir * (arm * 0.3);
    painter.line_segment([base, tip], stroke);
    let perp = vec2(-dir.y, dir.x) * (arm * 0.25);
    painter.line_segment([tip, tip - dir * (arm * 0.35) + perp], stroke);
    painter.line_segment([tip, tip - dir * (arm * 0.35) - perp], stroke);
}

/// A part of `rect` in the glyph's own frame: `along` runs the way `dir` points, `across` at
/// right angles to it, both as fractions of the rectangle's own size.
///
/// A glyph that points one way is the same glyph turned, so everything it is built out of is
/// stated once, in *along* and *across*, and this is the only place that knows which of those is
/// x and which is y.
fn part(rect: Rect, dir: Dir, along: RangeInclusive<f32>, across: RangeInclusive<f32>) -> Rect {
    let span = |from: f32, range: &RangeInclusive<f32>, size: f32| {
        (from + range.start() * size, from + range.end() * size)
    };
    match dir {
        Dir::Down => {
            let (top, bottom) = span(rect.top(), &along, rect.height());
            let (left, right) = span(rect.left(), &across, rect.width());
            Rect::from_min_max(pos2(left, top), pos2(right, bottom))
        }
        Dir::Right => {
            let (left, right) = span(rect.left(), &along, rect.width());
            let (top, bottom) = span(rect.top(), &across, rect.height());
            Rect::from_min_max(pos2(left, top), pos2(right, bottom))
        }
        Dir::Left => {
            // `along` is measured from the edge the glyph points at, which for this one is the
            // right, so the fractions count leftwards from there.
            let left = rect.right() - along.end() * rect.width();
            let right = rect.right() - along.start() * rect.width();
            let (top, bottom) = span(rect.top(), &across, rect.height());
            Rect::from_min_max(pos2(left, top), pos2(right, bottom))
        }
    }
}

#[cfg(test)]
mod tests {
    use egui::{Rect, pos2};

    use super::{Dir, corners, part};

    /// A square whose halves and quarters land on whole numbers, so that the two ways of naming
    /// a point — a fraction of the rectangle, and `egui`'s own `lerp` — agree exactly and the
    /// assertions below are about the geometry rather than about float error.
    fn square() -> Rect {
        Rect::from_min_max(pos2(10.0, 20.0), pos2(26.0, 36.0))
    }

    /// The chevron was two files' worth of corner literals before the glyphs were gathered, and
    /// is now three `triangle` calls over [`part`]. Same points, in the same
    /// order — the order matters, because a filled polygon is feathered along its winding, so a
    /// triangle listed the other way round is a different half-pixel at its edge.
    #[test]
    fn a_downward_chevron_draws_the_points_it_used_to() {
        let r = square();

        assert_eq!(
            corners(part(r, Dir::Down, 0.0..=0.5, 0.0..=1.0), Dir::Down),
            [r.left_top(), r.right_top(), r.center()],
            "the head"
        );
        assert_eq!(
            corners(part(r, Dir::Down, 0.5..=1.0, 0.0..=1.0), Dir::Down),
            [r.left_center(), r.right_center(), r.center_bottom()],
            "the chevron behind it"
        );
        assert_eq!(
            corners(part(r, Dir::Down, 0.5..=0.75, 0.25..=0.75), Dir::Down),
            [
                r.left_center().lerp(r.right_center(), 0.25),
                r.left_center().lerp(r.right_center(), 0.75),
                r.center().lerp(r.center_bottom(), 0.5),
            ],
            "the notch cut out of it"
        );
    }

    /// The same, a quarter turn round: the window's expand button.
    #[test]
    fn a_rightward_chevron_draws_the_points_it_used_to() {
        let r = square();

        assert_eq!(
            corners(part(r, Dir::Right, 0.0..=0.5, 0.0..=1.0), Dir::Right),
            [r.left_top(), r.center(), r.left_bottom()],
            "the head"
        );
        assert_eq!(
            corners(part(r, Dir::Right, 0.5..=1.0, 0.0..=1.0), Dir::Right),
            [r.center_top(), r.right_center(), r.center_bottom()],
            "the chevron behind it"
        );
        assert_eq!(
            corners(part(r, Dir::Right, 0.5..=0.75, 0.25..=0.75), Dir::Right),
            [
                r.center_top().lerp(r.center_bottom(), 0.25),
                r.center().lerp(r.right_center(), 0.5),
                r.center_top().lerp(r.center_bottom(), 0.75),
            ],
            "the notch cut out of it"
        );
    }

    /// What the gathering found: the arrow on a collapsed tab bar and the arrow on a strip
    /// against the left edge were two functions drawing one triangle. Stated as a test so that
    /// the day they are meant to differ, this says so.
    #[test]
    fn a_collapsed_bar_and_a_left_hand_strip_wear_the_same_arrow() {
        let r = square();

        assert_eq!(
            corners(r, Dir::Right),
            [r.left_top(), r.right_center(), r.left_bottom()]
        );
    }

    /// `part` measures from the edge the glyph points at, so the near half of a leftward glyph is
    /// its right-hand half. Nothing draws one yet; the arm exists so that the frame is a frame
    /// and not two special cases, and an untested third case is how it would quietly stop being.
    #[test]
    fn a_leftward_part_counts_from_the_right() {
        let r = square();

        assert_eq!(
            part(r, Dir::Left, 0.0..=0.5, 0.0..=1.0),
            Rect::from_min_max(r.center_top(), r.right_bottom())
        );
    }
}
