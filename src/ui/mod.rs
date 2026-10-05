mod combo_box;
mod completion_menu;
pub mod overlay;
mod signature_help;
pub mod status_bar;
pub mod terminal;

pub use overlay::*;
pub use status_bar::*;
pub use terminal::*;

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
