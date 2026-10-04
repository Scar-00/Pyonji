//! Find a running session and bring it back into the workspace.

use std::ops::Range;

use crate::{
    Next, Prev, PyTheme as _, Pyonji, Submit, assets::PyonjiAsset, terminal::SessionId, util,
};
use gpui::{prelude::*, *};
use gpui_base::{Button, actions::Cancel, h_flex, v_flex};
use gpui_component::{
    Icon, IconName, WindowExt,
    input::{Input, InputEvent, InputState},
};

const CONTEXT: &str = "SessionsView";
const ROW_HEIGHT: f32 = 48.0;

#[derive(Action, Clone, PartialEq)]
#[action(no_json)]
struct AttachToTab(usize);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", Next, Some(CONTEXT)),
        KeyBinding::new("up", Prev, Some(CONTEXT)),
        KeyBinding::new("enter", Submit, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
    ]);
    cx.bind_keys(
        (0..9).map(|tab| {
            KeyBinding::new(&format!("alt-{}", tab + 1), AttachToTab(tab), Some(CONTEXT))
        }),
    );
}

#[derive(Clone)]
struct Entry {
    id: SessionId,
    title: String,
    tab: Option<usize>,
}

pub struct SessionsView {
    pyonji: WeakEntity<Pyonji>,
    detached_only: bool,
    focus_handle: FocusHandle,
    selected: Option<SessionId>,
    search_input: Entity<InputState>,
    scroll_handle: UniformListScrollHandle,
    error: Option<&'static str>,
}

impl SessionsView {
    pub fn new(
        py: &WeakEntity<Pyonji>,
        detached_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Find by name or session number…"));
        cx.subscribe(&search_input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.selected = None;
                this.error = None;
                this.scroll_handle
                    .scroll_to_item(0, ScrollStrategy::Nearest);
                cx.notify();
            }
        })
        .detach();
        let focus_handle = cx.focus_handle();
        cx.on_focus_in(&focus_handle, window, |this, window, cx| {
            if this.focus_handle.is_focused(window) {
                window.focus(&this.search_input.focus_handle(cx), cx);
            }
        })
        .detach();
        Self {
            pyonji: py.clone(),
            detached_only,
            focus_handle,
            selected: None,
            search_input,
            scroll_handle: UniformListScrollHandle::new(),
            error: None,
        }
    }

    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_search(window, cx);
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.selected = None;
        self.error = None;
        self.scroll_handle
            .scroll_to_item(0, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn entries(&self, cx: &App) -> Vec<Entry> {
        let py = util::read!(self.pyonji, cx);
        let mut entries = py
            .detached_sessions
            .iter()
            .filter_map(|id| {
                let session = py.session_manager.session(*id)?;
                Some(Entry {
                    id: *id,
                    title: session.title().to_string(),
                    tab: None,
                })
            })
            .collect::<Vec<_>>();
        if !self.detached_only {
            for (index, tab) in py.tabs.iter().enumerate() {
                let Some(tab) = tab else { continue };
                entries.extend(tab.sessions().into_iter().filter_map(|id| {
                    let session = py.session_manager.session(id)?;
                    Some(Entry {
                        id,
                        title: session.title().to_string(),
                        tab: Some(index),
                    })
                }));
            }
        }
        entries
    }

    fn visible(&self, cx: &App) -> Vec<Entry> {
        let query = self.search_input.read(cx).value().trim().to_lowercase();
        self.entries(cx)
            .into_iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.title.to_lowercase().contains(&query)
                    || entry.id.to_string().contains(&query)
            })
            .collect()
    }

    fn navigate(&mut self, backwards: bool, cx: &mut Context<Self>) {
        let entries = self.visible(cx);
        let index = self
            .selected
            .and_then(|id| entries.iter().position(|entry| entry.id == id));
        let next = match (entries.len(), index, backwards) {
            (0, _, _) => None,
            (_, Some(index), true) => Some(index.saturating_sub(1)),
            (len, Some(index), false) => Some((index + 1).min(len - 1)),
            (len, None, true) => Some(len - 1),
            (_, None, false) => Some(0),
        };
        self.selected = next.map(|index| entries[index].id);
        if let Some(index) = next {
            self.scroll_handle
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
        self.error = None;
        cx.notify();
    }

    fn activate(
        &mut self,
        id: SessionId,
        destination: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Resolve ownership on activation: a session can close or move while
        // the dialog is open, and its old tab may now be empty.
        let activated =
            self.pyonji
                .update(cx, |py, cx| {
                    if py.session_manager.session(id).is_none() {
                        return false;
                    }
                    if py.detached_sessions.contains(&id) {
                        return py.attach_session(id, destination, cx);
                    }
                    let owner = py.tabs.iter().position(|tab| {
                        tab.as_ref().is_some_and(|tab| tab.sessions().contains(&id))
                    });
                    let Some(owner) = owner else { return false };
                    py.tabs[owner].as_mut().unwrap().set_active_session(id);
                    py.switch_tab(owner, cx)
                })
                .unwrap_or(false);
        if activated {
            window.close_dialog(cx);
        } else {
            self.error = Some("This session is no longer available. Choose another session.");
            cx.notify();
        }
    }

    fn on_submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.selected {
            self.activate(id, None, window, cx);
        }
    }

    fn on_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_input.read(cx).value().is_empty() {
            window.close_dialog(cx);
        } else {
            self.clear_search(window, cx);
            window.focus(&self.search_input.focus_handle(cx), cx);
        }
    }

    fn render_header(
        &self,
        shown: usize,
        total: usize,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let focused = self.search_input.focus_handle(cx).is_focused(window);
        v_flex()
            .flex_shrink_0()
            .gap_2()
            .px_4()
            .pt_3()
            .pb_2()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(16.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(if self.detached_only {
                                "Detached sessions"
                            } else {
                                "Sessions"
                            }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(12.))
                            .text_color(theme.text_muted)
                            .child(if shown == total {
                                total.to_string()
                            } else {
                                format!("{shown} / {total}")
                            }),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(px(36.))
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(if focused {
                        theme.focus_ring
                    } else {
                        theme.border
                    })
                    .child(
                        Icon::new(IconName::Search)
                            .size_4()
                            .text_color(theme.text_muted),
                    )
                    .child(
                        Input::new(&self.search_input)
                            .flex_1()
                            .bordered(false)
                            .appearance(false),
                    ),
            )
    }

    fn render_row(&self, entry: &Entry, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let selected = self.selected == Some(entry.id);
        let id = entry.id;
        Button::new(format!("session-{id}"))
            .accessibility_label(format!(
                "{} session {id}: {}",
                if entry.tab.is_none() {
                    "Attach"
                } else {
                    "Switch to"
                },
                entry.title
            ))
            .tab_stop(false)
            .focusable(false)
            .w_full()
            .h(px(ROW_HEIGHT))
            .border_l_2()
            .border_color(if selected {
                theme.selected_border
            } else {
                rgba(0)
            })
            .bg(if selected { theme.selected } else { rgba(0) })
            .hover(|style| {
                style.bg(if selected {
                    theme.selected
                } else {
                    theme.hovered
                })
            })
            .focus_visible(|style| style.border_color(theme.focus_ring).bg(theme.selected))
            .child(
                h_flex()
                    .size_full()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .child(
                        Icon::new(PyonjiAsset::Terminal)
                            .size_4()
                            .flex_shrink_0()
                            .text_color(if selected {
                                theme.accent
                            } else {
                                theme.text_muted
                            }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text)
                                    .child(entry.title.clone()),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_3()
                                    .text_size(px(11.))
                                    .text_color(theme.text_muted)
                                    .child(format!("Session {id}"))
                                    .when(!self.detached_only || entry.tab.is_some(), |this| {
                                        this.child(
                                            div()
                                                .text_color(if entry.tab.is_none() {
                                                    theme.accent
                                                } else {
                                                    theme.text_muted
                                                })
                                                .child(
                                                    entry
                                                        .tab
                                                        .map(|tab| format!("Tab {}", tab + 1))
                                                        .unwrap_or_else(|| "Detached".into()),
                                                ),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .w_4()
                            .flex_shrink_0()
                            .text_size(px(13.))
                            .text_color(if selected { theme.accent } else { rgba(0) })
                            .child("⏎"),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.activate(id, None, window, cx)))
    }

    fn render_list(&self, entries: Vec<Entry>, total: usize, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        if entries.is_empty() {
            let heading = if total > 0 {
                "No matching sessions"
            } else if self.detached_only {
                "No detached sessions"
            } else {
                "No running sessions"
            };
            let detail = if total > 0 {
                "Try another name or press Esc to clear the search."
            } else if self.detached_only {
                "Detach a session to keep it running outside your tabs."
            } else {
                "Open a tab to start a new session."
            };
            return v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .px_6()
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(theme.text)
                        .child(heading),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.text_muted)
                        .child(detail),
                )
                .into_any_element();
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .py_2()
            .child(
                uniform_list(
                    "sessions-list",
                    entries.len(),
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        entries[range]
                            .iter()
                            .map(|entry| this.render_row(entry, cx).into_any_element())
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll_handle)
                .size_full(),
            )
            .into_any_element()
    }

    fn render_footer(&self, entries: &[Entry], cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let entry = self
            .selected
            .and_then(|id| entries.iter().find(|entry| entry.id == id));
        let attach = entry.is_some_and(|entry| entry.tab.is_none());
        let py = util::read!(self.pyonji, cx);
        let destination = entry.and_then(|entry| entry.tab.or(py.current_tab));
        v_flex()
            .flex_shrink_0()
            .gap_2()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(theme.border)
            .when_some(self.error, |this, error| {
                this.child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.error)
                        .child(error),
                )
            })
            .when_some(destination, |this, tab| {
                this.child(
                    h_flex()
                        .flex_wrap()
                        .items_center()
                        .gap_4()
                        .child(hint(
                            "Enter",
                            format!(
                                "{} to tab {}",
                                if attach { "Attach" } else { "Switch" },
                                tab + 1
                            ),
                            theme,
                        ))
                        .when(attach, |this| {
                            this.child(hint("Alt 1–9", "Attach to tab", theme))
                        }),
                )
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_4()
                    .when(entry.is_some(), |this| {
                        this.child(hint("↑↓", "Choose", theme))
                    })
                    .child(hint(
                        "Esc",
                        if self.search_input.read(cx).value().is_empty() {
                            "Close"
                        } else {
                            "Clear search"
                        },
                        theme,
                    )),
            )
    }

    fn on_attach_to_tab(
        &mut self,
        action: &AttachToTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.selected
            && util::read!(self.pyonji, cx).detached_sessions.contains(&id)
        {
            self.activate(id, Some(action.0), window, cx);
        }
    }
}

impl Render for SessionsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let all = self.entries(cx);
        let py = util::read!(self.pyonji, cx);
        let font: SharedString = py
            .font_family
            .clone()
            .unwrap_or_else(|| "Iosevka".into())
            .into();
        let total = all.len();
        let entries = self.visible(cx);
        if !self
            .selected
            .is_some_and(|id| entries.iter().any(|entry| entry.id == id))
        {
            self.selected = entries.first().map(|entry| entry.id);
            if self.selected.is_some() {
                self.scroll_handle
                    .scroll_to_item(0, ScrollStrategy::Nearest);
            }
        }
        v_flex()
            .id("sessions-view")
            .key_context(CONTEXT)
            .tab_group()
            .track_focus(&self.focus_handle)
            .size_full()
            .font_family(font)
            .overflow_hidden()
            .rounded_lg()
            .bg(cx.theme().surface)
            .border_1()
            .border_color(cx.theme().border)
            .on_action(cx.listener(|this, _: &Next, _, cx| this.navigate(false, cx)))
            .on_action(cx.listener(|this, _: &Prev, _, cx| this.navigate(true, cx)))
            .on_action(cx.listener(Self::on_submit))
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(Self::on_attach_to_tab))
            .child(self.render_header(entries.len(), total, window, cx))
            .child(self.render_list(entries.clone(), total, cx))
            .child(self.render_footer(&entries, cx))
    }
}

impl Focusable for SessionsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn hint(
    keys: &'static str,
    label: impl Into<SharedString>,
    theme: &crate::Theme,
) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_2()
        .text_size(px(11.))
        .child(div().text_color(theme.text).child(keys))
        .child(div().text_color(theme.text_muted).child(label.into()))
}
