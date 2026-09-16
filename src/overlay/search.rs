//! Shared searchable overlay dialog built on gpui_component::Command.

use std::rc::Rc;

use gpui::{AnyElement, App, Entity, SharedString, Window, div, prelude::*};
use gpui_component::{
    IndexPath,
    command::{Command, CommandItem, CommandState},
};

use crate::{
    Surface,
    theme::{self, role},
};

use super::host::Overlay;

#[derive(Clone)]
pub struct SearchItem {
    pub label: String,
    pub keywords: Vec<String>,
}

impl SearchItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            keywords: Vec::new(),
        }
    }

    pub fn with_keywords(mut self, keywords: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }
}

/// `row`, current search `query`, then window/app.
type ConfirmFn = Rc<dyn Fn(usize, String, &mut Window, &mut App)>;

/// Open a searchable list dialog with shared Overlay chrome.
pub fn open_search_dialog(
    title: Option<String>,
    placeholder: impl Into<String>,
    footer: Option<&'static str>,
    items: Vec<SearchItem>,
    surface: Entity<Surface>,
    on_confirm: impl 'static + Fn(usize, String, &mut Window, &mut App),
    window: &mut Window,
    cx: &mut App,
) {
    let placeholder = placeholder.into();
    let state = cx.new(|cx| CommandState::new(window, cx));
    state.update(cx, |state, cx| state.set_query("", window, cx));
    let on_confirm: ConfirmFn = Rc::new(on_confirm);

    Overlay::present(surface, window, cx, move |mut dialog, window, cx| {
        let focus_state = state.clone();
        window.defer(cx, move |window, cx| {
            focus_state.update(cx, |state, cx| {
                state.focus(window, cx);
            });
        });

        if let Some(title) = title.clone() {
            dialog = dialog.title(title);
        }
        if let Some(text) = footer {
            dialog = dialog.footer(muted_footer(text));
        }

        let items = items.clone();
        let on_confirm = on_confirm.clone();
        let state = state.clone();
        let placeholder = placeholder.clone();
        dialog.content(move |content, _, _| {
            let on_confirm = on_confirm.clone();
            let state_for_confirm = state.clone();
            let mut command = Command::new(&state)
                .placeholder(placeholder.clone())
                .bordered(true)
                .on_confirm(move |index: IndexPath, window, cx| {
                    let query = state_for_confirm.read(cx).query(cx).to_string();
                    on_confirm(index.row, query, window, cx);
                });
            for item in &items {
                let mut row = CommandItem::new().label(item.label.clone());
                if !item.keywords.is_empty() {
                    row = row.keywords(item.keywords.clone());
                }
                command = command.item(row);
            }
            content.child(command)
        })
    });
}

fn muted_footer(text: impl Into<SharedString>) -> AnyElement {
    div()
        .w_full()
        .px(theme::space::_3)
        .py(theme::space::_2)
        .text_sm()
        .text_color(role::muted())
        .child(text.into())
        .into_any_element()
}
