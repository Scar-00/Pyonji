use gpui::{App, Context, IntoElement, Render, Window, anchored, deferred, div, prelude::*};

use crate::{
    theme::{self, role},
    ui::list_row::list_row,
};

/// Completion menu component: constrained popover anchored above the bar.
///
/// Owns its items and selection; renders nothing when empty so callers can
/// always include it.
pub struct CompletionMenu {
    items: Vec<String>,
    selected: usize,
}

impl CompletionMenu {
    pub fn new(items: Vec<String>, selected: usize) -> Self {
        Self { items, selected }
    }
}

impl Render for CompletionMenu {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        completion_menu(&self.items, self.selected)
    }
}

/// Constrained completion popover anchored above the status bar.
pub fn completion_menu(items: &[String], selected: usize) -> impl IntoElement {
    if items.is_empty() {
        return div().into_any_element();
    }
    let selected = selected.min(items.len() - 1);
    deferred(
        anchored()
            .anchor(gpui::Anchor::BottomLeft)
            .child(
                div()
                    .id("completion-menu")
                    .max_w(theme::size::COMPLETION_MAX_W)
                    .rounded(theme::radius::LG)
                    .border_1()
                    .border_color(role::menu_border())
                    .bg(role::menu_bg())
                    .py(theme::space::_1)
                    .px(theme::space::_1)
                    .children(items.iter().enumerate().map(|(index, hint)| {
                        list_row(hint.clone(), index == selected)
                    })),
            ),
    )
    .priority_auto()
    .into_any_element()
}

#[allow(dead_code)]
fn _list_row_compat(hint: String, selected: bool) -> impl IntoElement {
    list_row(hint, selected)
}

#[allow(dead_code)]
fn _app(_: &App) {}
