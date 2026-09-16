//! Pyonji UI theme tokens — Catppuccin Mocha.
//!
//! All painted chrome (status bar, completion menu, overlay lists, input
//! frames) should use these constants so colors and spacing can be tuned in
//! one place without touching component logic.

use gpui::{Pixels, Rgba, px, rgb};

/// Catppuccin Mocha palette (hex without alpha).
#[allow(dead_code)]
pub mod color {
    pub const ROSEWATER: u32 = 0xf5e0dc;
    pub const FLAMINGO: u32 = 0xf2cdcd;
    pub const PINK: u32 = 0xf5c2e7;
    pub const MAUVE: u32 = 0xcba6f7;
    pub const RED: u32 = 0xf38ba8;
    pub const MAROON: u32 = 0xeba0ac;
    pub const PEACH: u32 = 0xfab387;
    pub const YELLOW: u32 = 0xf9e2af;
    pub const GREEN: u32 = 0xa6e3a1;
    pub const TEAL: u32 = 0x94e2d5;
    pub const SKY: u32 = 0x89dceb;
    pub const SAPPHIRE: u32 = 0x74c7ec;
    pub const BLUE: u32 = 0x89b4fa;
    pub const LAVENDER: u32 = 0xb4befe;
    pub const TEXT: u32 = 0xcdd6f4;
    pub const SUBTEXT1: u32 = 0xbac2de;
    pub const SUBTEXT0: u32 = 0xa6adc8;
    pub const OVERLAY2: u32 = 0x9399b2;
    pub const OVERLAY1: u32 = 0x7f849c;
    pub const OVERLAY0: u32 = 0x6c7086;
    pub const SURFACE2: u32 = 0x585b70;
    pub const SURFACE1: u32 = 0x45475a;
    pub const SURFACE0: u32 = 0x313244;
    pub const BASE: u32 = 0x1e1e2e;
    pub const MANTLE: u32 = 0x181825;
    pub const CRUST: u32 = 0x11111b;

    /// Window / root background (slightly off mantle for contrast).
    pub const WINDOW: u32 = 0x181818;
}

/// Corner radii in logical pixels.
#[allow(dead_code)]
pub mod radius {
    use super::*;

    pub const SM: Pixels = px(4.0);
    pub const MD: Pixels = px(6.0);
    pub const LG: Pixels = px(8.0);
}

/// Spacing scale in logical pixels.
#[allow(dead_code)]
pub mod space {
    use super::*;

    pub const PX: Pixels = px(1.0);
    pub const _0_5: Pixels = px(2.0);
    pub const _1: Pixels = px(4.0);
    pub const _2: Pixels = px(8.0);
    pub const _3: Pixels = px(12.0);
    pub const _4: Pixels = px(16.0);
    pub const _6: Pixels = px(24.0);
}

/// Shared layout sizes for menus and overlay lists.
pub mod size {
    use super::*;

    pub const COMPLETION_MAX_W: Pixels = px(480.0);
    pub const OVERLAY_LIST_MAX_H: Pixels = px(320.0);
}

#[inline]
pub fn paint(hex: u32) -> Rgba {
    rgb(hex)
}

/// Semantic roles used by UI chrome.
pub mod role {
    use super::{color, paint};
    use gpui::Rgba;

    pub fn window_bg() -> Rgba {
        paint(color::WINDOW)
    }

    pub fn bar_bg() -> Rgba {
        paint(color::BASE)
    }

    pub fn surface() -> Rgba {
        paint(color::SURFACE0)
    }

    pub fn text() -> Rgba {
        paint(color::TEXT)
    }

    pub fn muted() -> Rgba {
        paint(color::SUBTEXT0)
    }

    pub fn accent() -> Rgba {
        paint(color::LAVENDER)
    }

    pub fn accent_fg() -> Rgba {
        paint(color::BASE)
    }

    pub fn message() -> Rgba {
        paint(color::MAUVE)
    }

    pub fn message_fg() -> Rgba {
        paint(color::CRUST)
    }

    pub fn input_bg() -> Rgba {
        paint(color::BASE)
    }

    pub fn input_border() -> Rgba {
        paint(color::SURFACE0)
    }

    pub fn input_border_focus() -> Rgba {
        paint(color::LAVENDER)
    }

    pub fn menu_bg() -> Rgba {
        paint(color::BASE)
    }

    pub fn menu_border() -> Rgba {
        paint(color::SURFACE0)
    }
}
