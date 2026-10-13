//! Looks that are a few style calls long and recur across screens, as
//! builder methods any element can take.

use gpui::{App, BoxShadow, InteractiveElement, Styled, hsla, px};

use crate::ui::theme::{PyTheme as _, Theme};

/// Surfaces and faces. Implemented for every [`Styled`] element.
pub trait PyStyled: Styled + Sized {
    /// Floating chrome: the translucent fill, the blur behind it, and the
    /// hairline it is drawn with when it has a border.
    fn glass(self, cx: &App) -> Self {
        let theme = cx.theme();
        self.bg(theme.glass)
            .border_color(theme.border)
            .backdrop_blur(Theme::GLASS_BLUR)
    }

    /// [`PyStyled::glass`] lifted off the window: dialogs and notifications.
    fn floating(self, cx: &App) -> Self {
        self.glass(cx).shadow(surface_shadow())
    }

    /// [`PyStyled::glass`] drawn as a small bordered card anchored to
    /// something else: completion lists, signature help.
    fn popover(self, cx: &App) -> Self {
        self.flex_none()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .glass(cx)
    }

    /// The terminal grid's own face, for anything that is typed or read like
    /// a command line.
    fn mono(self) -> Self {
        self.font_family(Theme::MONO_FONT)
    }
}

impl<E: Styled> PyStyled for E {}

/// States that answer to the pointer and the keyboard. Implemented for every
/// element that is both [`Styled`] and [`InteractiveElement`].
pub trait PyInteractive: Styled + InteractiveElement + Sized {
    /// A selectable row in a list. Selection is a rule and a raised fill, so
    /// it cannot be mistaken for hover; the same mark in every list.
    ///
    /// Sets the hover style, so the row must not set one of its own.
    fn list_row(self, selected: bool, cx: &App) -> Self {
        let theme = cx.theme();
        let row = self.border_l_2().cursor_pointer();
        if selected {
            row.border_color(theme.selected_border).bg(theme.selected)
        } else {
            let hovered = theme.hovered;
            row.border_color(Theme::TRANSPARENT)
                .hover(move |style| style.bg(hovered))
        }
    }

    /// Something clickable that is quieter than a button: a suggestion, a
    /// link, a "clear filter". A hairline that takes the accent under the
    /// pointer and the focus ring under the keyboard.
    ///
    /// Sets the hover style, so the element must not set one of its own.
    fn outlined(self, cx: &App) -> Self {
        let theme = cx.theme();
        let (accent, ring) = (theme.accent, theme.focus_ring);
        self.border_1()
            .border_color(theme.border)
            .cursor_pointer()
            .hover(move |style| style.border_color(accent))
            .focus_visible(move |style| style.border_color(ring))
    }
}

impl<E: Styled + InteractiveElement> PyInteractive for E {}

fn surface_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow::new(px(0.), px(2.), hsla(0., 0., 0., 0.06)).blur_radius(px(3.))]
}
