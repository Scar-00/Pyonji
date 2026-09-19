use std::ops::Range;

use crate::{PyTheme as _, Pyonji, terminal::SessionId, util};
use gpui::{prelude::FluentBuilder as _, *};
use gpui_base::*;
use gpui_component::{WindowExt, separator::Separator};

actions!([Submit, Next, Prev]);

pub struct SessionsView {
    pyonji: WeakEntity<Pyonji>,

    focus_handle: FocusHandle,
    selected: Option<usize>,
}

impl SessionsView {
    pub fn new(py: &WeakEntity<Pyonji>, cx: &mut Context<Self>) -> Self {
        cx.bind_keys([
            KeyBinding::new("enter", Submit, None),
            KeyBinding::new("up", Prev, None),
            KeyBinding::new("down", Next, None),
        ]);
        Self {
            pyonji: py.clone(),

            focus_handle: cx.focus_handle(),
            selected: None,
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
        let select_color = gpui::red();
        Button::new(format!("{tab}-{sid}"))
            .border_1()
            .border_color(theme.unselected)
            .w_full()
            .h_16()
            .map(|this| if selected {
                this.bg(select_color)
            }else {
                this.bg(theme.surface)
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let py = util::read!(self.pyonji, cx);
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
                        Some((i, *id, session.title().to_string()))
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let sessions_len = sessions.len();
        v_flex()
            .track_focus(&self.focus_handle)
            .debug_focused(&self.focus_handle, window, cx)
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
                                    .child(
                                        Self::layout(*tab, title.clone(), *id, selected, cx)
                                    )
                            })
                            .collect()
                    }),
                )
                .size_full(),
            )
    }
}

impl Focusable for SessionsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
