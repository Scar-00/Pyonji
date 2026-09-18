//! Pyonji UI theme tokens — Catppuccin Mocha.
//!
//! All painted chrome (status bar, completion menu, overlay lists, input
//! frames) should use these constants so colors and spacing can be tuned in
//! one place without touching component logic.
//!
//! [`PyonjiTheme`] additionally publishes the palette as a GPUI global (see
//! [`PyonjiActiveTheme`]) and projects the shared subset onto the kept
//! `gpui-component` theme plus the `gpui-base` layer, so the styled views we
//! still borrow (`Root` dialog stack, `Command` palette, `Input`/`Editor`
//! views) follow the same palette.

use gpui::{App, Global, Hsla, Pixels, Rgba, px, rgb};

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

/// The palette as a GPUI global, built from the [`color`] constants above.
///
/// Plain `role::*` fns remain the call-site API for Pyonji-owned chrome;
/// this struct exists so the palette can also be projected onto the kept
/// component theme (and through it the base layer) in one place.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct PyonjiTheme {
    pub background: Hsla,
    pub surface: Hsla,
    pub foreground: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    pub accent: Hsla,
    pub accent_foreground: Hsla,
    pub border: Hsla,
    pub message: Hsla,
    pub message_foreground: Hsla,
}

impl Global for PyonjiTheme {}

impl PyonjiTheme {
    pub fn dark() -> Self {
        let hex = |hex: u32| gpui::rgb_to_hsla(gpui::rgb(hex));
        Self {
            background: hex(color::WINDOW),
            surface: hex(color::BASE),
            foreground: hex(color::TEXT),
            muted: hex(color::SURFACE0),
            muted_foreground: hex(color::OVERLAY2),
            accent: hex(color::LAVENDER),
            accent_foreground: hex(color::BASE),
            border: hex(color::SURFACE0),
            message: hex(color::MAUVE),
            message_foreground: hex(color::CRUST),
        }
    }

    /// Install the theme and project the shared subset onto the kept
    /// component theme (dialog/`Command`/`Input` chrome) plus the base
    /// layer underneath it.
    ///
    /// Call after `gpui_component::init(cx)` + `Theme::change(...)`, which
    /// own the initial global setup.
    pub fn apply(cx: &mut App) {
        let theme = Self::dark();
        cx.set_global(theme.clone());

        let component = gpui_component::Theme::global_mut(cx);
        component.background = theme.surface;
        component.foreground = theme.foreground;
        component.accent = theme.accent;
        component.accent_foreground = theme.accent_foreground;
        component.muted = theme.muted;
        component.muted_foreground = theme.muted_foreground;
        component.border = theme.border;
        component.popover = theme.surface;
        component.popover_foreground = theme.foreground;
        gpui_component::Theme::sync_base(cx);
    }
}

/// Access to the active [`PyonjiTheme`] through an application context.
///
/// Mirrors `gpui-component`'s `ActiveTheme`: implemented for `App` and
/// reached through deref from `Context`/`Window`, so `cx.pyonji()` works in
/// `render` and event handlers.
#[allow(dead_code)]
pub trait PyonjiActiveTheme {
    fn theme(&self) -> &PyonjiTheme;
}

impl PyonjiActiveTheme for App {
    #[inline(always)]
    fn theme(&self) -> &PyonjiTheme {
        self.global::<PyonjiTheme>()
    }
}
