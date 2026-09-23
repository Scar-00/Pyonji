use std::ops::Range;

use crate::{PyTheme as _, Pyonji, terminal::SessionId, util};
use gpui::{prelude::FluentBuilder as _, *};
use gpui_base::*;
use gpui_component::{
    IconName, WindowExt,
    input::{Input, InputState},
    separator::Separator,
};
use crate::{Next, Prev, Submit};

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Submit, Some(SessionsView::CONTEXT)),
        KeyBinding::new("up", Prev, Some(SessionsView::CONTEXT)),
        KeyBinding::new("down", Next, Some(SessionsView::CONTEXT)),
    ]);
}

pub struct SessionsView {
    pyonji: WeakEntity<Pyonji>,

    focus_handle: FocusHandle,
    selected: Option<usize>,
    search_input: Entity<InputState>,
}

impl SessionsView {
    pub const CONTEXT: &str = "SessionsView";

    pub fn new(py: &WeakEntity<Pyonji>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search sessions…"));
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

    fn on_submit(
        &mut self,
        tab: usize,
        id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        _ = self.pyonji.update(cx, |this, cx| {
            this.switch_tab(tab, cx);
            if let Some(tab) = &mut this.tabs[tab] {
                tab.set_active_session(id);
            }
            window.close_dialog(cx);
        });
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
            .border_1()
            .border_color(if selected {
                theme.selected_border.opacity(0.15)
            } else {
                theme.unselected_border.opacity(0.15)
            })
            .w_full()
            .h_16()
            .map(|this| {
                if selected {
                    this.bg(select_color)
                } else {
                    this.bg(theme.surface.opacity(0.15))
                }
            })
            .shadow_md()
            .child(
                v_flex()
                    .size_full()
                    .child(
                        h_flex()
                            .px_3()
                            .size_full()
                            .justify_between()
                            .child(title)
                            .child(format!("Tab: {tab}")),
                    )
                    .child(Separator::horizontal().h_0p5().w_full())
                    .child(h_flex().size_full()),
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
            .track_focus(&self.focus_handle)
            .size_full()
            .on_action(cx.listener({
                let sessions = sessions.clone();
                move |this, _: &Submit, window, cx| {
                    let Some(index) = this.selected else {
                        return;
                    };
                    let (tab, id, _) = sessions[index];
                    Self::on_submit(this, tab, id, window, cx);
                }
            }))
            .on_action(cx.listener(move |this, _: &Next, _, cx| {
                if sessions_len == 0 {
                    this.selected = None;
                    return;
                }
                let next = match this.selected {
                    None => 0,
                    Some(index) => (index + 1) % sessions_len,
                };
                this.selected = Some(next);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &Prev, _, cx| {
                if sessions_len == 0 {
                    this.selected = None;
                    return;
                }
                let prev = match this.selected {
                    None => sessions_len - 1,
                    Some(0) => sessions_len - 1,
                    Some(index) => (index - 1) % sessions_len,
                };
                this.selected = Some(prev);
                cx.notify();
            }))
            .child(
                h_flex().w_full().py_1().child(
                    Input::new(&self.search_input)
                        .prefix(IconName::Search)
                        .w_full()
                        .bordered(false)
                        .appearance(false)
                        .bg(cx.theme().background.opacity(0.15)),
                ),
            )
            .child(
                v_flex().flex_1().child(
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
                                    h_flex()
                                        .w_full()
                                        .py_2()
                                        .justify_center()
                                        .items_center()
                                        .child(Self::layout(*tab, title.clone(), *id, selected, cx))
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
