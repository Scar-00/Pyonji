//! Shared searchable overlay dialog built on gpui-component::Command.
//!
//! `SearchDialog` holds its own state — the `CommandState` query entity and
//! the item list — and implements `Render`, so each searchable screen owns
//! one as `Entity<SearchDialog>`, constructed once and reused. Title,
//! placeholder and footer are fixed per screen; items refresh on every
//! `show`. Confirmation arrives as a `SearchDialogEvent`, which the owning
//! screen subscribes to (like `CommandPromptEvent`), so no per-open
//! closures are built. `SearchItem` is plain data (label + keywords).

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, IntoElement, Render, SharedString, Window,
    div, prelude::*,
};
use gpui_base::IndexPath;
use gpui_component::command::{Command, CommandItem, CommandState};

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

/// Events emitted by [`SearchDialog`]. The owning screen subscribes once at
/// construction and reads its own current state when confirming, so entries
/// can never go stale between opens.
#[derive(Clone, Debug)]
pub enum SearchDialogEvent {
    Confirm { row: usize, query: String },
}

/// Searchable list dialog content: owns the query state entity and the
/// items. Constructed once per screen; `show` refreshes the items, resets
/// and focuses the query, and presents with shared `Overlay` chrome.
pub struct SearchDialog {
    title: Option<String>,
    placeholder: String,
    footer: Option<String>,
    items: Vec<SearchItem>,
    query: Entity<CommandState>,
}

impl SearchDialog {
    pub(crate) fn new(
        title: Option<String>,
        placeholder: String,
        footer: Option<String>,
        query: Entity<CommandState>,
    ) -> Self {
        Self {
            title,
            placeholder,
            footer,
            items: Vec::new(),
            query,
        }
    }

    /// Refresh the items, reset and focus the query, and present with
    /// shared `Overlay` chrome. The dialog entity itself is reused — only
    /// its state changes.
    pub(crate) fn show(
        dialog: Entity<Self>,
        items: Vec<SearchItem>,
        surface: Entity<Surface>,
        window: &mut Window,
        cx: &mut App,
    ) {
        dialog.update(cx, |dialog, cx| {
            dialog.items = items;
            cx.notify();
        });
        let query = dialog.read(cx).query.clone();
        query.update(cx, |state, cx| state.set_query("", window, cx));
        window.defer(cx, move |window, cx| {
            query.update(cx, |state, cx| {
                state.focus(window, cx);
            });
        });
        let presented = dialog.clone();
        Overlay::present(surface, window, cx, move |mut shell, _, cx| {
            if let Some(title) = presented.read(cx).title.clone() {
                shell = shell.title(title);
            }
            if let Some(footer) = presented.read(cx).footer.clone() {
                shell = shell.footer(muted_footer(footer));
            }
            let view = presented.clone();
            shell.content(move |content, _, _| content.child(view.clone()))
        });
    }
}

impl EventEmitter<SearchDialogEvent> for SearchDialog {}

impl Render for SearchDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let query_for_confirm = self.query.clone();
        let mut command = Command::new(&self.query)
            .placeholder(self.placeholder.clone())
            .bordered(true)
            .on_confirm(move |index: IndexPath, _window, cx| {
                let query = query_for_confirm.read(cx).query(cx).to_string();
                this.update(cx, |_, cx| {
                    cx.emit(SearchDialogEvent::Confirm {
                        row: index.row,
                        query,
                    });
                });
            });
        for item in &self.items {
            let mut row = CommandItem::new().label(item.label.clone());
            if !item.keywords.is_empty() {
                row = row.keywords(item.keywords.clone());
            }
            command = command.item(row);
        }
        command.into_any_element()
    }
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
