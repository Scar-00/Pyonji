mod combo_box;
mod completion_menu;
pub mod overlay;
mod signature_help;
pub mod status_bar;
pub mod terminal;

pub use overlay::*;
pub use status_bar::*;
pub use terminal::*;

use crate::PyTheme as _;
use gpui::{ParentElement as _, SharedString, Styled as _, div, px};
use gpui_base::h_flex;

/// Give bare inputs the same selection color as the styled component inputs.
pub(crate) fn text_input(
    state: &gpui::Entity<gpui_base::input::InputState>,
    cx: &mut gpui::App,
) -> gpui_base::input::Input {
    let selection = gpui_component::Theme::global(cx).selection;
    state.update(cx, |input, _| {
        input.set_editor_style(gpui_base::input::InputEditorStyle {
            selection,
            ..Default::default()
        });
    });
    gpui_base::input::Input::new(state)
}

pub(crate) fn surface_shadow() -> Vec<gpui::BoxShadow> {
    vec![
        gpui::BoxShadow::new(gpui::px(0.), gpui::px(2.), gpui::hsla(0., 0., 0., 0.06))
            .blur_radius(gpui::px(3.)),
    ]
}

/// The terminal grid's own face, for anything that is typed or read like a
/// command line.
pub(crate) fn mono(cx: &gpui::App) -> SharedString {
    gpui_component::ActiveTheme::theme(cx)
        .mono_font_family
        .clone()
}

/// One key and what it does, the way every overlay footer spells it: the key
/// in the grid's face, the action muted and lower case.
pub(crate) fn hint(
    keys: impl Into<SharedString>,
    action: impl Into<SharedString>,
    cx: &gpui::App,
) -> gpui::Div {
    let theme = cx.theme();
    h_flex()
        .flex_shrink_0()
        .items_center()
        .gap_1p5()
        .text_size(px(11.0))
        .child(
            div()
                .font_family(mono(cx))
                .text_color(theme.text)
                .child(keys.into()),
        )
        .child(div().text_color(theme.text_muted).child(action.into()))
}
