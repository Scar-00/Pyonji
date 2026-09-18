use gpui::{IntoElement, SharedString, div, prelude::*};

use crate::theme::{self, role};

/// One selectable row used by completion menus and overlay lists.
///
/// Snapshot view (no own state): constructed per-frame from entity state,
/// like `ChatHistory` in t3chat. Holds an owned label + selection flag.
#[derive(IntoElement)]
pub struct ListRow {
    label: SharedString,
    selected: bool,
}

impl ListRow {
    pub fn new(label: impl Into<SharedString>, selected: bool) -> Self {
        Self {
            label: label.into(),
            selected,
        }
    }
}

impl RenderOnce for ListRow {
    fn render(self, _window: &mut gpui::Window, _cx: &mut gpui::App) -> impl IntoElement {
        div()
            .w_full()
            .px(theme::space::_2)
            .py(theme::space::_1)
            .rounded(theme::radius::SM)
            .when(self.selected, |this| {
                this.bg(role::accent()).text_color(role::accent_fg())
            })
            .when(!self.selected, |this| this.text_color(role::text()))
            .child(self.label)
    }
}

/// One selectable row used by completion menus and overlay lists.
#[allow(dead_code)]
pub fn list_row(label: impl Into<SharedString>, selected: bool) -> ListRow {
    ListRow::new(label, selected)
}
