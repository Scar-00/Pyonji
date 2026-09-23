use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::InputEvent;
use gpui_base::input::InputState;
use gpui_base::*;

use crate::PyTheme as _;
use crate::Pyonji;
use crate::terminal::SessionId;
use crate::util;

actions!([DismissStatusBarState, HistoryPrev, HistoryNext]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(
                "escape",
                DismissStatusBarState,
                Some(StatusBar::CONTEXT),
            ),
            KeyBinding::new(
                "up",
                HistoryNext,
                Some(StatusBar::CONTEXT),
            ),
            KeyBinding::new(
                "down",
                HistoryPrev,
                Some(StatusBar::CONTEXT),
            )
    ]);
}

#[allow(dead_code)]
pub enum StatusBarMode {
    Sessions,
    Cmd,
    Lua,
    Rename { inital: String, session: SessionId },
}

type Mode = StatusBarMode;

pub enum StatusBarEvent {
    Dismiss,
    Renamed(SessionId, String),
    ExecLua(String),
}

type Event = StatusBarEvent;

impl EventEmitter<Event> for StatusBar {}

pub struct StatusBar {
    pyonji: WeakEntity<Pyonji>,

    focus_handle: FocusHandle,
    mode: Mode,

    lua_history: HistoryManager,
}

impl StatusBar {
    pub const CONTEXT: &str = "STATUS-BAR";

    pub fn new(pyonji: WeakEntity<Pyonji>, cx: &mut Context<Self>) -> Self {
        Self {
            pyonji,
            focus_handle: cx.focus_handle(),
            mode: Mode::Sessions,

            lua_history: HistoryManager::new(),
        }
    }

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    pub fn push_lua_history(&mut self, code: String) {
        self.lua_history.push(code);
    }

    pub fn reset_history(&mut self) {
        self.lua_history.index = 0;
    }

    fn on_dismiss(
        &mut self,
        _: &DismissStatusBarState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_mode(Mode::Sessions, cx);
        let focus_handle = util::read!(self.pyonji, cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
    }
}

impl Focusable for StatusBar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for StatusBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .id("status-bar")
            .key_context(Self::CONTEXT)
            .on_action(cx.listener(Self::on_dismiss))
            .w_full()
            .px_1()
            .py_0p5()
            .bg(theme.surface)
            .text_color(theme.text)
            .items_center()
            .backdrop_blur(px(16.0))
            .map(|this| match &self.mode {
                Mode::Sessions => this.child(SessionView::new(&self.pyonji)),
                Mode::Rename { inital, session } => {
                    this.child(RenameView::new(&self.pyonji, inital.clone(), *session))
                }
                Mode::Lua => this.child(LuaView::new(&self.pyonji)),
                _ => this,
            })
            .into_any_element()
    }
}

#[derive(IntoElement)]
struct SessionView {
    pyonji: WeakEntity<Pyonji>,
}

impl SessionView {
    fn new(pyonji: &WeakEntity<Pyonji>) -> Self {
        Self {
            pyonji: pyonji.clone(),
        }
    }
}

impl RenderOnce for SessionView {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let py = util::read!(self.pyonji, cx);
        h_flex()
            .gap_1()
            .children(py.tabs.iter().enumerate().filter_map(|(i, tab)| {
                let id = tab.as_ref().and_then(|tab| tab.active_session())?;
                let title = py.session_manager.session(id)?.title();
                let label = format!("[{i}] - {title}");
                Some(
                    h_flex()
                        .items_center()
                        .justify_center()
                        .px_1()
                        .map(|this| {
                            if py.current_tab == Some(i) {
                                this.bg(theme.selected).font_bold()
                            } else {
                                this.bg(theme.unselected)
                            }
                        })
                        .child(label),
                )
            }))
    }
}

#[derive(IntoElement)]
struct RenameView {
    pyonji: WeakEntity<Pyonji>,
    inital: String,
    session: SessionId,
}

impl RenameView {
    fn new(pyonji: &WeakEntity<Pyonji>, inital: String, session: SessionId) -> Self {
        Self {
            pyonji: pyonji.clone(),
            inital,
            session,
        }
    }
}

impl RenderOnce for RenameView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let session = self.session;
        let py = self.pyonji.clone();
        let input_state = window.use_state(cx, |window, cx| {
            let mut state = InputState::new(window, cx);
            state.insert(self.inital, window, cx);
            cx.on_focus_out(&state.focus_handle(cx), window, {
                let py = py.clone();
                move |_, _, _, cx| {
                    let status_bar = util::read!(py, cx).status_bar.clone();
                    AppContext::emit(cx, &status_bar, Event::Dismiss);
                }
            })
            .detach();
            cx.subscribe_self(move |this, ev: &InputEvent, cx| {
                let InputEvent::PressEnter { .. } = ev else {
                    return;
                };
                let val = this.value();
                let status_bar = util::read!(py, cx).status_bar.clone();
                AppContext::emit(cx, &status_bar, Event::Renamed(session, val.to_string()));
                AppContext::emit(cx, &status_bar, Event::Dismiss);
            })
            .detach();
            state.focus(window, cx);
            state
        });

        h_flex().w_full().child(Input::new(&input_state))
    }
}

#[derive(IntoElement)]
struct LuaView {
    pyonji: WeakEntity<Pyonji>,
}

impl LuaView {
    fn new(pyonji: &WeakEntity<Pyonji>) -> Self {
        Self {
            pyonji: pyonji.clone(),
        }
    }
}

impl RenderOnce for LuaView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let py = self.pyonji.clone();
        let input_state = window.use_state(cx, |window, cx| {
            let state = InputState::new(window, cx).placeholder("Lua repl");
            cx.on_focus_out(&state.focus_handle(cx), window, {
                let py = py.clone();
                move |this, _, window, cx| {
                    let status_bar = util::read!(py, cx).status_bar.clone();
                    AppContext::emit(cx, &status_bar, Event::Dismiss);
                    this.set_value("", window, cx);
                }
            })
            .detach();
            cx.subscribe_in(&cx.entity(), window, {
                let py = py.clone();
                move |this, _, ev: &InputEvent, window, cx| {
                    let InputEvent::PressEnter { .. } = ev else {
                        return;
                    };
                    let val = this.value();
                    let status_bar = util::read!(py, cx).status_bar.clone();
                    AppContext::emit(cx, &status_bar, Event::ExecLua(val.to_string()));
                    AppContext::emit(cx, &status_bar, Event::Dismiss);
                    this.set_value("", window, cx);
                }
            })
            .detach();
            state.focus(window, cx);
            state
        });

        let on_next = window.listener_for(&input_state, {
            let py = py.clone();
            move |this, _: &HistoryNext, window, cx| {
                let value = {
                    let status_bar = util::read!(py, cx).status_bar.clone();
                    status_bar.update(cx, |this, _| {
                        let history = this
                            .lua_history
                            .history
                            .get(this.lua_history.index)
                            .cloned()?;
                        this.lua_history.index = this
                            .lua_history
                            .index
                            .saturating_add(1)
                            .min(this.lua_history.history.len());
                        Some(history)
                    })
                };
                if let Some(value) = value {
                    this.set_value(value, window, cx);
                }
            }
        });
        let on_prev = window.listener_for(&input_state, {
            let py = py.clone();
            move |this, _: &HistoryPrev, window, cx| {
                let value = {
                    let status_bar = util::read!(py, cx).status_bar.clone();
                    status_bar.update(cx, |this, _| {
                        let history = this
                            .lua_history
                            .history
                            .get(this.lua_history.index)
                            .cloned()?;
                        this.lua_history.index = this.lua_history.index.saturating_sub(1);
                        Some(history)
                    })
                };
                if let Some(value) = value {
                    this.set_value(value, window, cx);
                }
            }
        });
        h_flex()
            .on_action(on_next)
            .on_action(on_prev)
            .w_full()
            .child(Input::new(&input_state))
    }
}

struct HistoryManager {
    history: Vec<String>,
    index: usize,
}

impl HistoryManager {
    fn new() -> Self {
        Self {
            history: vec![],
            index: 0,
        }
    }

    fn push(&mut self, code: String) {
        self.history.push(code);
    }
}
