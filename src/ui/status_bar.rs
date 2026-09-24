use async_compat::CompatExt as _;
use async_lsp::ServerSocket;
use async_lsp::lsp_types::CompletionItem;
use async_lsp::lsp_types::CompletionTextEdit;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::InputEvent;
use gpui_base::input::InputState;
use gpui_base::*;

use crate::Next;
use crate::Prev;
use crate::PyTheme as _;
use crate::Pyonji;
use crate::lua_complete::LspClient;
use crate::terminal::SessionId;
use crate::util;

actions!([
    DismissStatusBarState,
    HistoryPrev,
    HistoryNext,
    LuaAcceptCompletion
]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", DismissStatusBarState, Some(StatusBar::CONTEXT)),
        KeyBinding::new("down", Next, Some(StatusBar::CONTEXT)),
        KeyBinding::new("up", Prev, Some(StatusBar::CONTEXT)),
        KeyBinding::new("tab", LuaAcceptCompletion, Some(StatusBar::CONTEXT)),
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

    //lsp
    lua_lsp: Option<LspClient>,
    selected: Option<usize>,
    scroll_handle: UniformListScrollHandle,
    items: Option<Vec<CompletionItem>>,
}

impl StatusBar {
    pub const CONTEXT: &str = "STATUS-BAR";

    pub fn new(pyonji: WeakEntity<Pyonji>, cx: &mut Context<Self>) -> Self {
        Self::spawn_lsp(cx);
        Self {
            pyonji,
            focus_handle: cx.focus_handle(),
            mode: Mode::Sessions,

            lua_history: HistoryManager::new(),
            lua_lsp: None,
            selected: None,
            scroll_handle: UniformListScrollHandle::new(),
            items: None,
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
        self.items = None;
    }

    fn spawn_lsp(cx: &mut Context<Self>) {
        cx.spawn(async |this, cx| {
            match LspClient::start("rust-analyzer", cx).compat().await {
                Ok(client) => {
                    _ = this.update(cx, |this, cx| {
                        this.lua_lsp = Some(client);
                        cx.notify();
                    });
                    tracing::warn!("lua-language-server ready");
                }
                Err(e) => tracing::error!("lua-language-server start failed: {e:?}"),
            }
        })
        .detach();
    }

    fn on_dismiss(
        &mut self,
        _: &DismissStatusBarState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.items = None;
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
                Mode::Lua => {
                    let view = LuaView::new(&self.pyonji, self.items.clone() ,self.selected.clone(), self.scroll_handle.clone())
                        .on_next(cx.listener(|this, _, _, cx| {
                            let Some(items) = this.items.as_ref() else {
                                this.selected = None;
                                cx.notify();
                                return;
                            };
                            let len = items.len();
                            if len == 0 {
                                this.selected = None;
                                return;
                            }
                            let next = match this.selected {
                                None => 0,
                                Some(index) => (index + 1) % len,
                            };
                            this.selected = Some(next);
                            this.scroll_handle.scroll_to_item(next, ScrollStrategy::Nearest);
                            cx.notify();
                        }))
                        .on_prev(cx.listener(|this, _, _, cx| {
                            let Some(items) = this.items.as_ref() else {
                                this.selected = None;
                                cx.notify();
                                return;
                            };
                            let len = items.len();
                            if len == 0 {
                                this.selected = None;
                                return;
                            }
                            let prev = match this.selected {
                                None => len - 1,
                                Some(0) => len - 1,
                                Some(index) => (index - 1) % len,
                            };
                            this.scroll_handle.scroll_to_item(prev, ScrollStrategy::Nearest);
                            this.selected = Some(prev);
                            cx.notify();
                        }));
                    this.child(view)
                },
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

    on_next: Option<Box<dyn Fn(&Next, &mut Window, &mut App)>>,
    on_prev: Option<Box<dyn Fn(&Prev, &mut Window, &mut App)>>,

    selected: Option<usize>,
    scroll_handle: UniformListScrollHandle,
    items: Option<Vec<CompletionItem>>,
}

impl LuaView {
    fn new(pyonji: &WeakEntity<Pyonji>, items: Option<Vec<CompletionItem>>, selected: Option<usize>, scroll_handle: UniformListScrollHandle) -> Self {
        Self {
            pyonji: pyonji.clone(),

            on_next: None,
            on_prev: None,

            selected,
            scroll_handle,
            items
        }
    }

    fn on_next(mut self, f: impl 'static + Fn(&Next, &mut Window, &mut App)) -> Self {
        self.on_next = Some(Box::new(f));
        self
    }

    fn on_prev(mut self, f: impl 'static + Fn(&Prev, &mut Window, &mut App)) -> Self {
        self.on_prev = Some(Box::new(f));
        self
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
                move |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => {
                        let val = this.value();
                        let len = val.len();
                        let status_bar = util::read!(py, cx).status_bar.clone();
                        let index = this.cursor();
                        let chars = val.chars().collect::<Vec<_>>();
                        let Some(&trigger) = chars.get(index.saturating_sub(1)) else {
                            status_bar.update(cx, |this, cx| {
                                this.items = None;
                                cx.notify();
                            });
                            return;
                        };
                        status_bar.update(cx, |this, cx| {
                            let Some(lsp) = this.lua_lsp.as_mut() else {
                                return;
                            };
                            _ = lsp.push_changes(0..len, val.as_str());
                            let server = lsp.server.clone();
                            let uri = lsp.uri.clone();
                            cx.spawn(async move |this, cx| -> Result<()> {
                                let items = LspClient::get_completions_for(server, uri, trigger, index as u32).compat().await?;
                                this.update(cx, |this, cx| {
                                    this.items = items;
                                    cx.notify();
                                })
                            })
                            .detach();
                        });
                    }
                    InputEvent::PressEnter { .. } => {
                        let val = this.value();
                        let status_bar = util::read!(py, cx).status_bar.clone();
                        AppContext::emit(cx, &status_bar, Event::ExecLua(val.to_string()));
                        AppContext::emit(cx, &status_bar, Event::Dismiss);
                        this.set_value("", window, cx);
                    }
                    _ => {}
                }
            })
            .detach();
            state.focus(window, cx);
            state
        });

        let edit = self.selected.clone().and_then(|selected| {
            self
                .items
                .as_ref()
                .and_then(|items| items.get(selected).cloned())
        });

        let selected = self.selected.clone();

        v_flex()
            .key_context(StatusBar::CONTEXT)
            .when_some(self.on_next, |this, a| this.on_action(a))
            .when_some(self.on_prev, |this, a| this.on_action(a))
            .on_action(window.listener_for(&input_state, move |_, _: &LuaAcceptCompletion, _, _| {
                if let Some(edit) = edit.clone() {
                    println!("item = {edit:#?}");
                }
            }))
            .w_full()
            .children(self.items.map(|items| {
                CompletionMenu::new(items, selected, self.scroll_handle)
            }))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .child(Input::new(&input_state)),
            )
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

#[derive(IntoElement)]
struct CompletionMenu {
    items: Vec<CompletionItem>,
    selected: Option<usize>,
    scroll_handle: UniformListScrollHandle,
}

impl CompletionMenu {
    fn new(items: Vec<CompletionItem>, selected: Option<usize>, scroll_handle: UniformListScrollHandle) -> Self {
        Self {
            items,
            selected,
            scroll_handle,
        }
    }

    fn render_item(item: &CompletionItem, selected: bool, cx: &mut App) -> Div {
        let theme = cx.theme();
        let kind = item
            .kind
            .map(|k| format!("{k:?}"))
            .unwrap_or_default();
        h_flex()
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .py_0p5()
                    .px_1()
                    .rounded_md()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .map(|this| {
                        if selected {
                            this.bg(theme.selected)
                        }else {
                            this
                        }
                    })
                    .child(
                        div()
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .child(item.label.clone()),
                    )
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .w_20()
                            .justify_start()
                            .text_color(theme.text_muted)
                            .font_extrabold()
                            .child(kind),
                    )
            )
    }
}

impl RenderOnce for CompletionMenu {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let Self { items, selected, scroll_handle } = self;
        deferred(anchored()
            .anchor(Anchor::BottomLeft)
            .offset(point(px(0.0), px(-8.0)))
            .child(
                v_flex()
                    .flex_none()
                    .border_1()
                    .border_color(gpui::white().opacity(0.5))
                    .rounded_xl()
                    .mb_1()
                    .h_48()
                    .min_w_128()
                    .max_w(px(480.0))
                    .overflow_hidden()
                    .bg(theme.surface_elevated.opacity(0.2))
                    .backdrop_blur(px(24.0))
                    .p_2()
                    .child(
                        uniform_list(
                            "completion-items-list",
                            items.len(),
                            move |range, _, cx|  {
                                let start = range.start;

                                items[range]
                                    .iter()
                                    .enumerate()
                                    .map(|(i, item)| {
                                        let i = i + start;
                                        Self::render_item(item, Some(i) == selected, cx)
                                    }).collect()
                            }
                        )
                        .track_scroll(&scroll_handle)
                        .with_sizing_behavior(ListSizingBehavior::Infer)
                        .with_horizontal_sizing_behavior(
                            ListHorizontalSizingBehavior::Unconstrained,
                        )
                        .h_full()
                    )
            ),
        )
        .priority_auto()
    }
}
