//! Geometry value types owned by the core model — deliberately **egui-free**.
//!
//! # Why these exist
//!
//! The core (tree, nodes, operations) must not depend on `egui`; that dependency is
//! enforced by a test gate, not by convention. Almost all geometry in this crate is
//! *derived* per frame by the layout pass and therefore lives outside the model
//! entirely (see [`crate::layout`]). What is left here is the small amount of geometry
//! that is genuine **state**:
//!
//! * where the user put a floating window (`WindowState`),
//! * where a detached tab should be placed (`TabDestination::Window`).
//!
//! # Wire compatibility (deliberate)
//!
//! Field names and order mirror `egui`'s `Pos2` / `Vec2` / `Rect` exactly
//! (`{x, y}` and `{min, max}`), so layouts persisted before core was split off still
//! deserialize into these types byte-for-byte identically. Do not rename the fields.
//!
//! Conversions to and from the `egui` types live in [`crate::layout::convert`] — the
//! `egui` side of the boundary — so this module stays importable from the core.

/// A point in screen coordinates. Wire-compatible with `egui::Pos2`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Point {
    /// Horizontal coordinate, growing rightwards.
    pub x: f32,
    /// Vertical coordinate, growing downwards.
    pub y: f32,
}

impl Point {
    /// The origin.
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    /// Construct a point from its coordinates.
    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A width/height pair. Wire-compatible with `egui::Vec2`.
///
/// Named `Size` rather than `Vec2` because the core only ever uses it as an extent;
/// it deliberately carries no vector algebra.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Size {
    /// Width.
    pub x: f32,
    /// Height.
    pub y: f32,
}

impl Size {
    /// Construct a size from width and height.
    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned rectangle. Wire-compatible with `egui::Rect`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Rect {
    /// Top-left corner.
    pub min: Point,
    /// Bottom-right corner.
    pub max: Point,
}

impl Rect {
    /// The inverted-infinite rectangle, mirroring `egui::Rect::NOTHING`: it contains no
    /// points and is the identity element of `union`.
    ///
    /// Kept bit-identical to egui's so that a value round-tripping through the two
    /// representations compares equal.
    pub const NOTHING: Self = Self {
        min: Point {
            x: f32::INFINITY,
            y: f32::INFINITY,
        },
        max: Point {
            x: f32::NEG_INFINITY,
            y: f32::NEG_INFINITY,
        },
    };

    /// Construct a rectangle from its top-left corner and its extent.
    #[inline]
    pub const fn from_min_size(min: Point, size: Size) -> Self {
        Self {
            min,
            max: Point {
                x: min.x + size.x,
                y: min.y + size.y,
            },
        }
    }

    /// Width of the rectangle. Negative for an inverted rectangle.
    #[inline]
    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    /// Height of the rectangle. Negative for an inverted rectangle.
    #[inline]
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    /// Extent of the rectangle.
    #[inline]
    pub fn size(&self) -> Size {
        Size::new(self.width(), self.height())
    }
}
