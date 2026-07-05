use std::fmt::Debug;

use derive_more::Deref;
use taffy::{Layout, LengthPercentageAuto};

use crate::renderer::Color;

#[derive(Debug, Clone, Copy, Deref)]
pub struct Pixel(f64);

impl From<f64> for Pixel {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

impl From<f32> for Pixel {
    fn from(value: f32) -> Self {
        Self(value as f64)
    }
}

#[derive(Debug, Clone)]
pub struct Point<T: Debug + Clone> {
    pub x: T,
    pub y: T,
}

impl<T: Debug + Clone> Point<T> {
    pub fn new(x: impl Into<T>, y: impl Into<T>) -> Self {
        Self {
            x: x.into(),
            y: y.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Size<T: Debug + Clone> {
    pub width: T,
    pub height: T,
}

impl From<Size<Sizing>> for taffy::Size<taffy::Dimension> {
    fn from(value: Size<Sizing>) -> Self {
        let width = match value.width {
            Sizing::Abs(abs) => taffy::Dimension::length(abs.0 as f32),
            Sizing::Fract(fract) => taffy::Dimension::percent(fract),
        };
        let height = match value.height {
            Sizing::Abs(abs) => taffy::Dimension::length(abs.0 as f32),
            Sizing::Fract(fract) => taffy::Dimension::percent(fract),
        };

        Self {
            width,
            height,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Rect {
    pub origin: Point<Pixel>,
    pub size: Size<Pixel>,
}

impl From<&Layout> for Rect {
    fn from(value: &Layout) -> Self {
        Self {
            origin: Point {
                x: value.location.x.into(),
                y: value.location.y.into(),
            },
            size: Size {
                width: value.size.width.into(),
                height: value.size.height.into(),
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Sizing {
    Abs(Pixel),
    Fract(f32),
}

#[derive(Debug, Clone, Copy)]
pub enum Positioning {
    Relative,
    Absolute,
}

#[derive(Default)]
pub struct Style {
    pub color: Option<Color>,
    pub sizeing: Option<Size<Sizing>>,
    pub positioning: Option<Positioning>,
    pub inset: Option<Point<Pixel>>,
}

impl Style {
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    pub fn pos(mut self, pos: Positioning) -> Self {
        self.positioning = Some(pos);
        self
    }

    pub fn size(mut self, size: Size<Sizing>) -> Self {
        self.sizeing = Some(size);
        self
    }

    pub fn inset(mut self, point: Point<Pixel>) -> Self {
        self.inset = Some(point);
        self
    }
}

impl From<&Style> for taffy::Style {
    fn from(value: &Style) -> Self {
        let mut style = Self::default();
        value.sizeing.as_ref().map(|size| style.size = size.clone().into());
        value.positioning.as_ref().map(|pos| match pos {
            Positioning::Absolute => style.position = taffy::Position::Absolute,
            Positioning::Relative => style.position = taffy::Position::Relative,
        });
        value.inset.as_ref().map(|inset| {
            style.inset.left = LengthPercentageAuto::length(*inset.x as f32);
            style.inset.bottom = LengthPercentageAuto::length(*inset.y as f32);
        });
        style
    }
}
