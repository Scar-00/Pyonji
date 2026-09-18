use gpui::{IntoElement, anchored, deferred, div, prelude::*};

use crate::{
    theme::{self, role},
    ui::list_row::ListRow,
};

/// Completion menu snapshot: constrained popover anchored above the bar.
///
/// Snapshot view (no own state): constructed per-frame from entity state,
/// like `ChatHistory` in t3chat. Renders nothing when empty so callers can
/// always include it.
#[derive(IntoElement)]
pub struct CompletionMenu {
    items: Vec<String>,
    selected: usize,
}

impl CompletionMenu {
    pub fn new(items: Vec<String>, selected: usize) -> Self {
        Self { items, selected }
    }
}

impl RenderOnce for CompletionMenu {
    fn render(self, _window: &mut gpui::Window, _cx: &mut gpui::App) -> impl IntoElement {
        if self.items.is_empty() {
            return div().into_any_element();
        }
        let selected = self.selected.min(self.items.len() - 1);
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
                        .children(self.items.into_iter().enumerate().map(|(index, hint)| {
                            ListRow::new(hint, index == selected)
                        })),
                ),
        )
        .priority_auto()
        .into_any_element()
    }
}

/// Constrained completion popover anchored above the status bar.
#[allow(dead_code)]
pub fn completion_menu(items: &[String], selected: usize) -> CompletionMenu {
    CompletionMenu::new(items.to_vec(), selected)
}
