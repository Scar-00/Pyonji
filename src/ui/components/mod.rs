//! The pieces Pyonji's screens are built from.
//!
//! Reach for these before writing styles by hand: they already wear the
//! right [`Theme`](super::Theme) tokens, so a new screen matches the old ones
//! without restating how.
//!
//! - [`PyStyled`] / [`PyInteractive`]: builder methods any element can take —
//!   `glass`, `floating`, `popover`, `mono`, `list_row`, `outlined`.
//! - [`panel_header`], [`panel_footer`], [`footer_status`], [`EmptyState`]:
//!   the frame of an overlay screen.
//! - [`SearchField`], [`editable_combo_box`], [`text_input`]: fields.
//! - [`hint`], [`rule`], [`enter_mark`]: small marks.
//! - [`toast`], [`status_icon`]: notifications.

mod combo_box;
mod marks;
mod panel;
mod search_field;
mod style;
mod toast;

pub use combo_box::editable_combo_box;
pub use marks::{enter_mark, hint, rule};
pub use panel::{EmptyState, footer_status, panel_footer, panel_header};
pub use search_field::SearchField;
pub use style::{PyInteractive, PyStyled};
pub use toast::{status_icon, toast};

/// A bare input that selects text in the same colour as the kit's styled
/// inputs.
pub fn text_input(
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
