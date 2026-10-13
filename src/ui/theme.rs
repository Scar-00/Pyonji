//! Pyonji's colours, and the one place they are decided.
//!
//! Views never spell a colour themselves: they read a token from [`Theme`]
//! through [`PyTheme::theme`], or reach for a component in
//! [`crate::ui::components`] that already knows which tokens it wears. The
//! component kit keeps its own theme; [`Theme::apply_component_theme`] writes
//! the tokens it shares with Pyonji's over it, so a kit button and a Pyonji
//! row agree on what the accent is.

use gpui::{
    App, ColorExt as _, FontWeight, Global, HighlightStyle, Pixels, ReadGlobal as _, Rgba,
    SharedString, Window, px, rgb, rgb_to_hsla,
};
use gpui_component::ThemeMode;

/// The one hue that means "this is the live thing": selection, focus, the
/// prompt, the running build.
const LILAC: u32 = 0xc9a7e8;
/// Search matches, and anything that wants attention without alarm.
const AMBER: u32 = 0xe0af68;
const LINE: u32 = 0x303030;

pub trait PyTheme {
    fn theme(&self) -> &Theme;
}

impl PyTheme for App {
    #[inline(always)]
    fn theme(&self) -> &Theme {
        Theme::global(self)
    }
}

impl Global for Theme {}

#[derive(serde::Serialize)]
pub struct Theme {
    // Base layers
    pub background: Rgba,
    pub surface: Rgba,
    pub surface_elevated: Rgba,
    /// The translucent fill floating chrome sits on: dialogs, popovers,
    /// notifications. Always paired with [`Theme::GLASS_BLUR`].
    pub glass: Rgba,
    /// A selected row on [`Theme::glass`], where an opaque
    /// [`Theme::selected`] fill would punch a hole in it.
    pub glass_selected: Rgba,

    // Text
    pub text: Rgba,
    pub text_muted: Rgba,
    pub text_disabled: Rgba,

    // Tabs / list items
    pub selected: Rgba,
    pub unselected: Rgba,
    pub hovered: Rgba,
    pub selected_border: Rgba,
    pub unselected_border: Rgba,

    // Accent
    pub accent: Rgba,
    pub accent_muted: Rgba,

    // Terminal-specific
    pub cursor: Rgba,
    pub cursor_text: Rgba,
    pub selection: Rgba,
    pub search_match: Rgba,
    pub search_match_active: Rgba,
    pub scrollbar: Rgba,
    pub scrollbar_hover: Rgba,
    pub split_divider: Rgba,
    pub split_divider_active: Rgba,

    // Semantic status
    pub success: Rgba,
    pub warning: Rgba,
    pub error: Rgba,
    pub info: Rgba,

    // Overlays / popups
    pub overlay_backdrop: Rgba,
    pub tooltip_background: Rgba,
    pub border: Rgba,
    pub focus_ring: Rgba,

    /// The family the terminal grid is drawn in, for the chrome's monospaced
    /// text. Follows the config through [`Theme::set_mono_font`].
    pub mono_font: SharedString,
}

impl Theme {
    /// A border or fill that should show nothing still needs a colour.
    pub const TRANSPARENT: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.0);

    /// How far floating chrome blurs what is behind its [`Theme::glass`].
    pub const GLASS_BLUR: Pixels = px(24.0);

    pub fn init(cx: &mut App) {
        cx.set_global(Theme::new());
    }

    /// The characters a query or a caret position picked out of a label.
    pub fn match_highlight(&self) -> HighlightStyle {
        HighlightStyle {
            color: Some(rgb_to_hsla(self.accent)),
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        }
    }

    /// Draw the chrome's monospaced text in `family`, the grid's own family,
    /// rather than whatever the platform's default mono happens to be.
    pub fn set_mono_font(family: SharedString, cx: &mut App) {
        cx.global_mut::<Self>().mono_font = family;
        Self::apply_component_overrides(cx);
        cx.refresh_windows();
    }

    pub fn apply_component_theme(window: &mut Window, cx: &mut App) {
        gpui_component::Theme::change(ThemeMode::Dark, Some(window), cx);
        Self::apply_component_overrides(cx);
    }

    /// The kit re-runs `Theme::change` on its own, when the system appearance
    /// is synced, and that restores its stock colours. Re-apply Pyonji's after
    /// every change to the kit's theme. Idempotent, so the write it makes does
    /// not trigger another round.
    pub fn keep_component_overrides(cx: &mut App) {
        cx.observe_global::<gpui_component::Theme>(Self::apply_component_overrides)
            .detach();
    }

    fn apply_component_overrides(cx: &mut App) {
        let theme = Self::global(cx);
        let selection = rgb_to_hsla(theme.selection);
        let accent = rgb_to_hsla(theme.accent);
        let on_accent = rgb_to_hsla(theme.background);
        let mono_font = theme.mono_font.clone();
        let current = gpui_component::Theme::global(cx);
        if current.tokens.button_primary.color == accent
            && current.mono_font_family == mono_font
            && current.selection == selection
        {
            return;
        }
        let component = gpui_component::Theme::global_mut(cx);
        component.selection = selection;
        component.mono_font_family = mono_font;
        // Primary buttons speak in the one accent, like every other mark of
        // "this is the live thing", instead of the kit's stock white.
        // The kit paints from `tokens` and reads the colour fields elsewhere,
        // so both copies are set.
        let hover = accent.opacity(0.88);
        let active = accent.opacity(0.76);
        component.primary = accent;
        component.primary_hover = hover;
        component.primary_active = active;
        component.primary_foreground = on_accent;
        component.button_primary = accent;
        component.button_primary_hover = hover;
        component.button_primary_active = active;
        component.button_primary_foreground = on_accent;
        component.ring = accent;
        let tokens = &mut component.tokens;
        tokens.primary = accent.into();
        tokens.primary_hover = hover.into();
        tokens.primary_active = active.into();
        tokens.primary_foreground = on_accent.into();
        tokens.button_primary = accent.into();
        tokens.button_primary_hover = hover.into();
        tokens.button_primary_active = active.into();
        tokens.button_primary_foreground = on_accent.into();
        tokens.ring = accent.into();
    }

    fn new() -> Self {
        let surface = rgb(0x202020);
        Self {
            // Base layers
            background: rgb(0x181818),
            surface,
            surface_elevated: rgb(0x2a2a2a),
            glass: surface.opacity(0.15),
            glass_selected: rgb(LILAC).opacity(0.12),

            // Text
            text: rgb(0xdcdcdc),
            text_muted: rgb(0xa0a0a0),
            text_disabled: rgb(0x5a5a5a),

            // Tabs / list items
            selected: rgb(0x2c2c2c),
            unselected: rgb(0x1c1c1c),
            hovered: rgb(0x242424),
            selected_border: rgb(LILAC),
            unselected_border: rgb(LINE),

            // Accent (one hue, used everywhere emphasis is needed)
            accent: rgb(LILAC),
            accent_muted: rgb(LILAC).opacity(0.25),

            // Terminal-specific
            cursor: rgb(0xdcdcdc),
            cursor_text: rgb(0x181818),
            selection: rgb(LILAC).opacity(0.3),
            search_match: rgb(AMBER).opacity(0.4),
            search_match_active: rgb(AMBER).opacity(0.8),
            scrollbar: rgb(0xffffff).opacity(0.12),
            scrollbar_hover: rgb(0xffffff).opacity(0.25),
            split_divider: rgb(LINE),
            split_divider_active: rgb(LILAC),

            // Semantic status (bell, exit codes, warnings, etc.)
            success: rgb(0x9ece6a),
            warning: rgb(AMBER),
            error: rgb(0xf7768e),
            info: rgb(0x7dcfff),

            // Overlays / popups
            overlay_backdrop: rgb(0x000000).opacity(0.5),
            tooltip_background: rgb(LINE),
            border: rgb(LINE),
            focus_ring: rgb(LILAC),

            // Replaced by the installed family once the config has loaded.
            mono_font: crate::renderer::DEFAULT_FAMILIES[0].into(),
        }
    }
}

#[cfg(test)]
mod theme_readability_tests {
    use super::Theme;
    use gpui::Rgba;

    fn luminance(color: Rgba) -> f32 {
        let channel = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.red) + 0.7152 * channel(color.green) + 0.0722 * channel(color.blue)
    }

    #[test]
    fn supporting_text_and_search_matches_remain_readable_on_ui_surfaces() {
        let theme = Theme::new();
        for background in [
            theme.background,
            theme.surface,
            theme.surface_elevated,
            theme.selected,
            theme.hovered,
        ] {
            for foreground in [
                theme.text,
                theme.text_muted,
                theme.accent,
                theme.search_match,
                theme.search_match_active,
            ] {
                // Search match tokens are rendered as opaque text, with an underline.
                let contrast = (luminance(foreground) + 0.05) / (luminance(background) + 0.05);
                assert!(contrast >= 4.5, "text contrast {contrast} is below 4.5:1");
            }
        }
    }
}
