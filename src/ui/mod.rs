mod combo_box;
mod completion_menu;
pub mod overlay;
mod signature_help;
pub mod status_bar;
pub mod terminal;

pub use overlay::*;
pub use status_bar::*;
pub use terminal::*;

pub(crate) fn surface_shadow() -> Vec<gpui::BoxShadow> {
    vec![
        gpui::BoxShadow::new(gpui::px(0.), gpui::px(2.), gpui::hsla(0., 0., 0., 0.06))
            .blur_radius(gpui::px(3.)),
    ]
}
