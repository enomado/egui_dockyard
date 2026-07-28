//! Conversions between the core's egui-free geometry ([`crate::core::geom`]) and
//! egui's own `Pos2` / `Vec2` / `Rect`.
//!
//! These live on the *egui side* of the boundary on purpose: the core module must stay
//! importable without egui, so it cannot name egui's types even to convert to them.

use egui::{Pos2, Rect as EguiRect, Vec2};

use crate::core::geom::{Point, Rect, Size};

impl From<Pos2> for Point {
    fn from(value: Pos2) -> Self {
        Self {
            x: value.x,
            y: value.y,
        }
    }
}

impl From<Point> for Pos2 {
    fn from(value: Point) -> Self {
        Pos2::new(value.x, value.y)
    }
}

impl From<Vec2> for Size {
    fn from(value: Vec2) -> Self {
        Self {
            x: value.x,
            y: value.y,
        }
    }
}

impl From<Size> for Vec2 {
    fn from(value: Size) -> Self {
        Vec2::new(value.x, value.y)
    }
}

impl From<EguiRect> for Rect {
    fn from(value: EguiRect) -> Self {
        Self {
            min: value.min.into(),
            max: value.max.into(),
        }
    }
}

impl From<Rect> for EguiRect {
    fn from(value: Rect) -> Self {
        EguiRect::from_min_max(value.min.into(), value.max.into())
    }
}

#[cfg(test)]
mod tests {
    use egui::{Pos2, Rect as EguiRect, Vec2};

    use crate::core::geom::{Point, Rect, Size};

    /// The core types exist to keep egui out of the model, not to change any value.
    /// A round trip through them must be bit-identical, otherwise persisted window
    /// placement would drift a little on every load/save cycle.
    #[test]
    fn round_trip_is_lossless() {
        let rect = EguiRect::from_min_max(Pos2::new(-3.5, 17.25), Pos2::new(1024.0, 768.5));
        assert_eq!(EguiRect::from(Rect::from(rect)), rect);

        let pos = Pos2::new(0.1, -0.2);
        assert_eq!(Pos2::from(Point::from(pos)), pos);

        let size = Vec2::new(12.5, 0.0);
        assert_eq!(Vec2::from(Size::from(size)), size);
    }

    /// `Rect::NOTHING` is the value a never-shown window carries, and the renderer
    /// compares against egui's own constant — the two must agree exactly, infinities
    /// included.
    #[test]
    fn nothing_matches_egui() {
        assert_eq!(EguiRect::from(Rect::NOTHING), EguiRect::NOTHING);
        assert_eq!(Rect::from(EguiRect::NOTHING), Rect::NOTHING);
    }

    /// `from_min_size` is used to place detached windows; it must agree with egui's.
    #[test]
    fn from_min_size_matches_egui() {
        let min = Point::new(10.0, 20.0);
        let size = Size::new(100.0, 50.0);
        assert_eq!(
            EguiRect::from(Rect::from_min_size(min, size)),
            EguiRect::from_min_size(min.into(), size.into())
        );
    }
}
