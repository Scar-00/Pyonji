//! One accessible editing ancestor for a search field and its result list.

use gpui::{prelude::*, *};
use gpui_base::{input::InputState, v_flex};

pub(super) fn editable_combo_box(
    id: &'static str,
    label: &'static str,
    placeholder: &'static str,
    input: &Entity<InputState>,
    expanded: bool,
    cx: &App,
) -> Stateful<Div> {
    let state = input.clone();
    v_flex()
        .id(id)
        .role(accesskit::Role::EditableComboBox)
        .track_focus(&input.focus_handle(cx))
        .aria_label(label)
        .aria_placeholder(placeholder)
        .aria_value(input.read(cx).value().to_string())
        .aria_expanded(expanded)
        .on_a11y_action(accesskit::Action::SetValue, move |data, window, cx| {
            if let Some(accesskit::ActionData::Value(value)) = data {
                state.update(cx, |input, cx| {
                    input.replace_all(value.to_string(), window, cx)
                });
            }
        })
}
