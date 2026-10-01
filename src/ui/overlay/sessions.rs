use std::ops::Range;

use crate::{logging::ResultLogExt as _, terminal::SessionId, util, PyTheme as _, Pyonji};
use crate::{Next, Prev, Submit};
use gpui::{prelude::FluentBuilder as _, *};
use gpui_base::input::InputEvent;
use gpui_base::*;
use gpui_component::{
    input::{Input, InputState},
    IconName, WindowExt,
};

const CONTEXT: &str = "SessionsView";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", Next, Some(CONTEXT)),
        KeyBinding::new("up", Prev, Some(CONTEXT)),
        KeyBinding::new("enter", Submit, Some(CONTEXT)),
    ])
}

pub struct SessionsView {
    pyonji: WeakEntity<Pyonji>,

    focus_handle: FocusHandle,
    selected: Option<usize>,
    search_input: Entity<InputState>,
}

impl SessionsView {
    pub fn new(py: &WeakEntity<Pyonji>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search sessions…"));
        cx.subscribe(&search_input, |this, _, ev, cx| {
            if !matches!(ev, InputEvent::Change) {
                return;
            }
            this.selected = Some(0);
            cx.notify();
        })
        .detach();
        let focus_handle = cx.focus_handle();
        cx.on_focus_in(&focus_handle, window, |this, window, cx| {
            let focus = this.search_input.focus_handle(cx).clone();
            window.focus(&focus, cx);
        })
        .detach();
        Self {
            pyonji: py.clone(),

            focus_handle,
            selected: None,
            search_input,
        }
    }

    fn on_next(&mut self, sessions_len: usize, cx: &mut Context<Self>) {
        if sessions_len == 0 {
            self.selected = None;
            cx.notify();
            return;
        }
        self.selected = Some(match self.selected {
            None => 0,
            Some(index) => (index + 1) % sessions_len,
        });
        cx.notify();
    }

    fn on_prev(&mut self, sessions_len: usize, cx: &mut Context<Self>) {
        if sessions_len == 0 {
            self.selected = None;
            cx.notify();
            return;
        }
        self.selected = Some(match self.selected {
            None => sessions_len - 1,
            Some(index) => (index + sessions_len - 1) % sessions_len,
        });
        cx.notify();
    }

    #[tracing::instrument(level = "warn", skip(self, window, cx))]
    fn on_submit(
        &mut self,
        tab: usize,
        id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pyonji
            .update(cx, |this, cx| {
                this.switch_tab(tab, cx);
                if let Some(tab) = &mut this.tabs[tab] {
                    tab.set_active_session(id);
                }
                window.close_dialog(cx);
            })
            .log();
    }

    fn layout(
        tab: usize,
        title: String,
        sid: SessionId,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        let theme = cx.theme();
        let select_color = theme.selected;
        Button::new(format!("{tab}-{sid}"))
            .border_l_2()
            .border_color(if selected {
                theme.selected_border
            } else {
                theme.unselected_border
            })
            .w_full()
            .h_12()
            .rounded_md()
            .map(|this| {
                if selected {
                    this.bg(select_color)
                } else {
                    this.bg(theme.surface.opacity(0.55))
                }
            })
            .child(
                h_flex()
                    .size_full()
                    .px_3()
                    .items_center()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(theme.text)
                                    .truncate()
                                    .child(title),
                            )
                            .child(
                                h_flex()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(format!("Session {}", sid)),
                            ),
                    )
                    .child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .items_center()
                            .rounded_sm()
                            .bg(theme.background.opacity(0.55))
                            .text_xs()
                            .text_color(if selected {
                                theme.accent
                            } else {
                                theme.text_muted
                            })
                            .child(format!("Tab {}", tab + 1)),
                    ),
            )
            .hover(|style| style.cursor_pointer())
            .on_click(
                cx.listener(move |this, _, window, cx| Self::on_submit(this, tab, sid, window, cx)),
            )
    }
}

impl Render for SessionsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let py = util::read!(self.pyonji, cx);
        let query = self.search_input.read(cx).value().to_lowercase();

        let sessions = py
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(i, tab)| tab.as_ref().map(|tab| (i, tab)))
            .flat_map(|(i, tab)| {
                tab.sessions()
                    .iter()
                    .filter_map(|id| {
                        let session = py.session_manager.session(*id)?;
                        let title = session.title().to_string();
                        if query.is_empty() || title.to_lowercase().contains(&query) {
                            Some((i, *id, title))
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let sessions_len = sessions.len();

        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener(move |this, _: &Next, _, cx| {
                Self::on_next(this, sessions_len, cx);
            }))
            .on_action(cx.listener(move |this, _: &Prev, _, cx| {
                Self::on_prev(this, sessions_len, cx);
            }))
            .on_action(cx.listener({
                let sessions = sessions.clone();
                move |this, _: &Submit, window, cx| {
                    if let Some((tab, id, _)) =
                        this.selected.and_then(|selected| sessions.get(selected))
                    {
                        Self::on_submit(this, *tab, *id, window, cx);
                    }
                }
            }))
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .px_3()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .h(px(40.0))
                            .px_3()
                            .items_center()
                            .rounded_md()
                            .bg(cx.theme().surface)
                            .border_1()
                            .border_color(cx.theme().border)
                            .child(
                                Input::new(&self.search_input)
                                    .prefix(IconName::Search)
                                    .flex_1()
                                    .bordered(false)
                                    .appearance(false),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .px_1()
                            .child(
                                h_flex()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().text_muted)
                                    .child("Sessions"),
                            )
                            .child(
                                h_flex()
                                    .text_xs()
                                    .text_color(cx.theme().text_disabled)
                                    .child(if sessions_len == 1 {
                                        "1 session"
                                    } else {
                                        "multiple sessions"
                                    }),
                            ),
                    ),
            )
            .child(
                v_flex().flex_1().px_2().pt_2().child(
                    uniform_list(
                        "sessions-list",
                        sessions.len(),
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            let start = range.start;
                            sessions[range]
                                .iter()
                                .enumerate()
                                .map(|(i, (tab, id, title))| {
                                    let selected = this.selected == Some(i + start);
                                    h_flex().w_full().py_1().child(Self::layout(
                                        *tab,
                                        title.clone(),
                                        *id,
                                        selected,
                                        cx,
                                    ))
                                })
                                .collect()
                        }),
                    )
                    .size_full(),
                ),
            )
    }
}

impl Focusable for SessionsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
