use gpui::{Context, IntoElement, Render, SharedString, Window, div, prelude::*};

use crate::theme::{self, role};

/// One selectable row used by completion menus and overlay lists.
///
/// Owns its label and selection so overlay lists and menus share one
/// component instead of ad-hoc divs.
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

impl Render for ListRow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        list_row(self.label.clone(), self.selected)
    }
}

/// One selectable row used by completion menus and overlay lists.
pub fn list_row(label: impl Into<SharedString>, selected: bool) -> impl IntoElement {
    let label = label.into();
    div()
        .w_full()
        .px(theme::space::_2)
        .py(theme::space::_1)
        .rounded(theme::radius::SM)
        .when(selected, |this| {
            this.bg(role::accent()).text_color(role::accent_fg())
        })
        .when(!selected, |this| this.text_color(role::text()))
        .child(label)
}
