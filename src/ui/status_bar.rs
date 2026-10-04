use async_compat::CompatExt as _;
use async_lsp::lsp_types::CompletionItem;
use async_lsp::lsp_types::CompletionTextEdit;
use async_lsp::lsp_types::InsertTextFormat;
use async_lsp::lsp_types::Position;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_base::input::InputEvent;
use gpui_base::input::InputState;
use gpui_base::*;
use std::collections::HashSet;
use std::ops::Range;

use super::completion_menu::CompletionMenu;
use super::signature_help::LuaSignature;

use crate::Next;
use crate::Prev;
use crate::PyTheme as _;
use crate::Pyonji;
use crate::lua_complete::{LspClient, default_command};
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

#[derive(Default)]
struct LuaCompletion {
    generation: u64,
    input: Option<(String, usize)>,
    selected: Option<usize>,
    items: Option<Vec<CompletionItem>>,
    resolved: HashSet<usize>,
    accepted_input: Option<(String, usize)>,
}

impl LuaCompletion {
    fn clear(&mut self) {
        self.generation += 1;
        self.input = None;
        self.selected = None;
        self.items = None;
        self.resolved.clear();
        self.accepted_input = None;
    }

    fn begin_request(&mut self, value: &str, cursor: usize) -> u64 {
        self.clear();
        self.input = Some((value.to_string(), cursor));
        self.generation
    }

    fn finish_request(&mut self, generation: u64, items: Option<Vec<CompletionItem>>) -> bool {
        if generation != self.generation || self.input.is_none() {
            return false;
        }
        self.items = items.filter(|items| !items.is_empty()).map(|mut items| {
            items.sort_by(|a, b| {
                a.sort_text
                    .as_deref()
                    .unwrap_or(&a.label)
                    .cmp(b.sort_text.as_deref().unwrap_or(&b.label))
            });
            items
        });
        self.selected = self.items.as_ref().map(|items| {
            items
                .iter()
                .position(|item| item.preselect == Some(true))
                .unwrap_or(0)
        });
        true
    }

    fn matches_input(&self, value: &str, cursor: usize) -> bool {
        self.input
            .as_ref()
            .is_some_and(|(text, position)| text == value && *position == cursor)
    }

    fn take_accepted_input(&mut self, value: &str, cursor: usize) -> bool {
        self.accepted_input
            .take()
            .is_some_and(|(text, position)| text == value && position == cursor)
    }

    fn selected_item(&self, value: &str, cursor: usize) -> Option<CompletionItem> {
        self.item(value, cursor, self.selected?)
    }

    fn item(&self, value: &str, cursor: usize, index: usize) -> Option<CompletionItem> {
        if !self.matches_input(value, cursor) {
            return None;
        }
        self.items.as_ref()?.get(index).cloned()
    }

    fn finish_resolve(&mut self, generation: u64, index: usize, item: CompletionItem) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some(current) = self.items.as_mut().and_then(|items| items.get_mut(index)) else {
            return false;
        };
        *current = item;
        true
    }
}

pub struct StatusBar {
    pyonji: WeakEntity<Pyonji>,

    focus_handle: FocusHandle,
    mode: Mode,

    lua_history: HistoryManager,

    //lsp
    lua_lsp: Option<LspClient>,
    completion: LuaCompletion,
    signature: LuaSignature,
    scroll_handle: UniformListScrollHandle,
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
            completion: LuaCompletion::default(),
            signature: LuaSignature::default(),
            scroll_handle: UniformListScrollHandle::new(),
        }
    }

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.completion.clear();
        self.signature.clear();
        self.mode = mode;
        cx.notify();
    }

    pub fn push_lua_history(&mut self, code: String) {
        self.lua_history.push(code);
    }

    pub fn reset_history(&mut self) {
        self.lua_history.index = 0;
    }

    pub fn clear_completion(&mut self, cx: &mut Context<Self>) {
        self.completion.clear();
        cx.notify();
    }

    fn spawn_lsp(cx: &mut Context<Self>) {
        cx.spawn(async |this, cx| {
            let path = default_command();
            match LspClient::start(path, cx).compat().await {
                Ok(client) => {
                    _ = this.update(cx, |this, cx| {
                        this.lua_lsp = Some(client);
                        cx.notify();
                    });
                }
                Err(e) => tracing::error!("lua-lsp failed to start: {e:?}"),
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
        if matches!(self.mode, Mode::Lua)
            && (self.completion.items.is_some() || self.signature.help.is_some())
        {
            self.clear_completion(cx);
            self.signature.dismiss();
            return;
        }
        self.set_mode(Mode::Sessions, cx);
        let focus_handle = util::read!(self.pyonji, cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
    }

    fn resolve_selected(&mut self, cx: &mut Context<Self>) {
        let Some(lsp) = self.lua_lsp.as_ref().filter(|lsp| lsp.resolve_completions) else {
            return;
        };
        let Some(index) = self.completion.selected else {
            return;
        };
        let Some(item) = self
            .completion
            .items
            .as_ref()
            .and_then(|items| items.get(index))
            .cloned()
        else {
            return;
        };
        if !self.completion.resolved.insert(index) {
            return;
        }
        let mut server = lsp.server.clone();
        let generation = self.completion.generation;
        cx.spawn(async move |this, cx| {
            match LspClient::resolve_item(&mut server, item).compat().await {
                Ok(item) => {
                    _ = this.update(cx, |this, cx| {
                        if this.completion.finish_resolve(generation, index, item) {
                            cx.notify();
                        }
                    });
                }
                Err(error) => tracing::debug!("failed to resolve Lua suggestion: {error}"),
            }
        })
        .detach();
    }

    fn update_signature(
        &mut self,
        value: &str,
        cursor: usize,
        input: WeakEntity<InputState>,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.mode, Mode::Lua) || self.signature.matches_input(value, cursor) {
            return;
        }
        let Some(lsp) = self.lua_lsp.as_ref().filter(|lsp| lsp.signature_help) else {
            return;
        };
        let server = lsp.server.clone();
        let uri = lsp.uri.clone();
        let generation = self.signature.begin_request(value, cursor);
        cx.notify();
        let Some(generation) = generation else {
            return;
        };
        let position = completion_position(value, cursor);
        cx.spawn(async move |this, cx| {
            match LspClient::get_signature_help(server, uri, position)
                .compat()
                .await
            {
                Ok(help) => {
                    _ = this.update(cx, |this, cx| {
                        let current = input.upgrade().is_some_and(|input| {
                            let input = input.read(cx);
                            this.signature
                                .matches_input(input.value().as_str(), input.cursor())
                        });
                        if current && this.signature.finish_request(generation, help) {
                            cx.notify();
                        }
                    });
                }
                Err(error) => tracing::debug!("failed to get Lua function arguments: {error}"),
            }
        })
        .detach();
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
            .min_h(px(32.0))
            .flex_shrink_0()
            .line_height(relative(1.5))
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
                    let view = LuaView::new(
                        &self.pyonji,
                        self.completion.items.clone(),
                        self.completion.selected,
                        self.scroll_handle.clone(),
                    )
                    .on_next(cx.listener(|this, _, _, cx| {
                        let Some(items) = this.completion.items.as_ref() else {
                            this.completion.selected = None;
                            cx.notify();
                            return;
                        };
                        let len = items.len();
                        if len == 0 {
                            this.completion.selected = None;
                            return;
                        }
                        let next = match this.completion.selected {
                            None => 0,
                            Some(index) => (index + 1) % len,
                        };
                        this.completion.selected = Some(next);
                        this.scroll_handle
                            .scroll_to_item(next, ScrollStrategy::Nearest);
                        this.resolve_selected(cx);
                        cx.notify();
                    }))
                    .on_prev(cx.listener(|this, _, _, cx| {
                        let Some(items) = this.completion.items.as_ref() else {
                            this.completion.selected = None;
                            cx.notify();
                            return;
                        };
                        let len = items.len();
                        if len == 0 {
                            this.completion.selected = None;
                            return;
                        }
                        let prev = match this.completion.selected {
                            None => len - 1,
                            Some(0) => len - 1,
                            Some(index) => (index - 1) % len,
                        };
                        this.scroll_handle
                            .scroll_to_item(prev, ScrollStrategy::Nearest);
                        this.completion.selected = Some(prev);
                        this.resolve_selected(cx);
                        cx.notify();
                    }));
                    this.child(view)
                }
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

    fn on_click(pyonji: WeakEntity<Pyonji>, tab: usize, _: &mut Window, cx: &mut App) {
        _ = pyonji.update(cx, |this, cx| {
            this.switch_tab(tab, cx);
        });
    }
}

impl RenderOnce for SessionView {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let py = util::read!(self.pyonji, cx);
        let pyonji = self.pyonji.clone();
        h_flex()
            .id("status-sessions-list")
            .flex_1()
            .min_w_0()
            .role(accesskit::Role::TabList)
            .aria_label("Terminal tabs")
            .gap_3()
            .overflow_x_scroll()
            .children(py.tabs.iter().enumerate().filter_map(move |(i, tab)| {
                let id = tab.as_ref().and_then(|tab| tab.active_session())?;
                let title = py.session_manager.session(id)?.title();
                let label = format!("[{}] - {title}", i + 1);
                let selected = py.current_tab == Some(i);
                let pyonji = pyonji.clone();
                Some(
                    h_flex()
                        .id(("sessions-label", i))
                        .role(accesskit::Role::Tab)
                        .tab_index(0)
                        .focus_visible(|style| style.text_color(theme.accent))
                        .aria_selected(selected)
                        .aria_label(label.clone())
                        .items_center()
                        .justify_center()
                        .px_1()
                        .map(|this| {
                            if selected {
                                this.bg(theme.selected)
                                    .font_bold()
                                    .border_b_2()
                                    .border_color(theme.accent)
                            } else {
                                this.bg(theme.unselected).pb_0p5()
                            }
                        })
                        .child(label)
                        .hover(|style| style.cursor_pointer())
                        .on_click(move |_, window, cx| {
                            Self::on_click(pyonji.clone(), i, window, cx);
                        }),
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
    fn new(
        pyonji: &WeakEntity<Pyonji>,
        items: Option<Vec<CompletionItem>>,
        selected: Option<usize>,
        scroll_handle: UniformListScrollHandle,
    ) -> Self {
        Self {
            pyonji: pyonji.clone(),

            on_next: None,
            on_prev: None,

            selected,
            scroll_handle,
            items,
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

    fn accept_completion(
        pyonji: &WeakEntity<Pyonji>,
        input: &mut InputState,
        clicked: Option<(u64, usize)>,
        window: &mut Window,
        cx: &mut Context<InputState>,
    ) {
        let value = input.value().to_string();
        let cursor = input.cursor().min(value.len());
        let Some(py) = pyonji.upgrade() else {
            return;
        };
        let status_bar = cx.read_entity(&py, |py, _| py.status_bar.clone());
        let item = status_bar.update(cx, |bar, cx| {
            if !matches!(bar.mode, Mode::Lua) {
                return None;
            }
            let item = match clicked {
                Some((generation, index)) if generation == bar.completion.generation => {
                    bar.completion.item(&value, cursor, index)
                }
                Some(_) => None,
                None => bar.completion.selected_item(&value, cursor),
            };
            if item.is_some() {
                bar.clear_completion(cx);
            }
            item
        });
        let Some(item) = item else {
            return;
        };
        let range = completion_range(&item, &value, cursor);
        let replacement = completion_replacement(&item);
        let new_cursor = range.start + replacement.len();
        let mut accepted_value = value;
        accepted_value.replace_range(range.clone(), &replacement);
        status_bar.update(cx, |bar, _| {
            // Programmatic replacements also emit Change. Sync the document
            // without immediately reopening the suggestion we just inserted.
            bar.completion.accepted_input = Some((accepted_value, new_cursor));
        });
        input.set_selected_range(range, cx);
        input.replace(replacement, window, cx);
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
                        let status_bar = util::read!(py, cx).status_bar.clone();
                        let index = this.cursor();
                        let input = cx.weak_entity();
                        status_bar.update(cx, |this, cx| {
                            let accepted = this.completion.take_accepted_input(val.as_str(), index);
                            this.clear_completion(cx);
                            if !matches!(this.mode, Mode::Lua) {
                                return;
                            }
                            let Some(lsp) = this.lua_lsp.as_mut() else {
                                return;
                            };
                            if let Err(error) = lsp.push_changes(val.as_str()) {
                                tracing::error!("failed to update Lua prompt document: {error}");
                                return;
                            }
                            let server = lsp.server.clone();
                            let uri = lsp.uri.clone();
                            this.update_signature(val.as_str(), index, input.clone(), cx);
                            if accepted || !should_request_completion(val.as_str(), index) {
                                return;
                            }
                            let position = completion_position(val.as_str(), index);
                            let generation = this.completion.begin_request(val.as_str(), index);
                            cx.spawn(async move |this, cx| -> Result<()> {
                                let items = LspClient::get_completions_for(server, uri, position)
                                    .compat()
                                    .await?;
                                this.update(cx, |this, cx| {
                                    let current = input.upgrade().is_some_and(|input| {
                                        let input = input.read(cx);
                                        this.completion
                                            .matches_input(input.value().as_str(), input.cursor())
                                    });
                                    if current && this.completion.finish_request(generation, items)
                                    {
                                        this.scroll_handle.scroll_to_item(
                                            this.completion.selected.unwrap_or(0),
                                            ScrollStrategy::Top,
                                        );
                                        this.resolve_selected(cx);
                                        cx.notify();
                                    }
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
            // InputEvent::Change covers edits; observing the input also covers
            // arrow keys and mouse caret movement. Snapshot checks ignore blink
            // and layout notifications, and keep Escape dismissal persistent.
            cx.observe_in(&cx.entity(), window, {
                let py = py.clone();
                move |this, _, _, cx| {
                    let value = this.value();
                    let cursor = this.cursor();
                    let input = cx.weak_entity();
                    let status_bar = util::read!(py, cx).status_bar.clone();
                    status_bar.update(cx, |bar, cx| {
                        if bar.completion.input.is_some()
                            && !bar.completion.matches_input(value.as_str(), cursor)
                        {
                            bar.clear_completion(cx);
                        }
                        bar.update_signature(value.as_str(), cursor, input, cx);
                    });
                }
            })
            .detach();
            state.focus(window, cx);
            state
        });

        let selected = self.selected;
        let py_accept = self.pyonji.clone();
        let input = input_state.read(cx);
        let value = input.value();
        let cursor = input.cursor().min(value.len());
        let fragment = value[word_start(value.as_str(), cursor)..cursor].to_string();
        let status_bar = util::read!(self.pyonji, cx).status_bar.clone();
        let bar = status_bar.read(cx);
        let completion = &bar.completion;
        let generation = completion.generation;
        let signature = bar
            .signature
            .help
            .clone()
            .filter(|_| bar.signature.matches_input(value.as_str(), cursor));
        // Cursor movement does not emit Change. Never display suggestions for
        // a different cursor location, even when the text is unchanged.
        let items = self
            .items
            .filter(|_| completion.matches_input(value.as_str(), cursor));
        let input_click = input_state.clone();
        let py_click = self.pyonji.clone();

        crate::ui::combo_box::editable_combo_box(
            "lua-search-results",
            "Lua repl",
            "Lua repl",
            &input_state,
            items.is_some(),
            cx,
        )
        .key_context(StatusBar::CONTEXT)
        .when_some(self.on_next, |this, a| this.on_action(a))
        .when_some(self.on_prev, |this, a| this.on_action(a))
        .on_action(window.listener_for(
            &input_state,
            move |input, _: &LuaAcceptCompletion, window, cx| {
                Self::accept_completion(&py_accept, input, None, window, cx);
            },
        ))
        .w_full()
        .when(items.is_some() || signature.is_some(), |view| {
            view.child({
                CompletionMenu::new(
                    items.unwrap_or_default(),
                    selected,
                    fragment,
                    self.scroll_handle,
                    move |index, window, cx| {
                        input_click.update(cx, |input, cx| {
                            Self::accept_completion(
                                &py_click,
                                input,
                                Some((generation, index)),
                                window,
                                cx,
                            );
                        });
                    },
                )
                .signature_help(signature)
            })
        })
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

/// Whether `c` continues a Lua identifier (`foo`, `bar2`, `_x`).
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// LSP positions count UTF-16 units; the input cursor counts UTF-8 bytes.
fn completion_position(value: &str, cursor: usize) -> Position {
    let cursor = value.floor_char_boundary(cursor.min(value.len()));
    let prefix = &value[..cursor];
    Position {
        line: prefix.bytes().filter(|byte| *byte == b'\n').count() as u32,
        character: prefix
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .encode_utf16()
            .count() as u32,
    }
}

/// Byte offset where the identifier fragment ending at `cursor` starts.
///
/// `cursor` is a UTF-8 byte offset into `value` (as reported by
/// [`InputState::cursor`]). Anything that is not an identifier char —
/// `.`, `:`, whitespace, brackets, … — terminates the fragment, so
/// `string.su|` yields the `su` range and `pr|` yields `pr`.
fn word_start(value: &str, cursor: usize) -> usize {
    let mut s = cursor.min(value.len());
    // Snap a mid-character cursor back to a boundary before scanning.
    if !value.is_char_boundary(s) {
        s = value.floor_char_boundary(s);
    }
    while s > 0 {
        let Some((idx, c)) = value
            .get(..s)
            .and_then(|prefix| prefix.chars().next_back())
            .map(|c| (s - c.len_utf8(), c))
        else {
            break;
        };
        if is_ident_char(c) {
            s = idx;
        } else {
            break;
        }
    }
    s
}

/// Automatic completion needs a name fragment or a member-access operator.
/// An invoked request at an empty fragment (for example after `)`) otherwise
/// asks LuaLS for every global and keyword available at that position.
fn should_request_completion(value: &str, cursor: usize) -> bool {
    let cursor = value.floor_char_boundary(cursor.min(value.len()));
    let prefix = &value[..cursor];
    if prefix.ends_with(['.', ':']) {
        return true;
    }
    value[word_start(value, cursor)..cursor]
        .chars()
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
}

/// Convert an LSP `character` (UTF-16 code units) into a byte offset into
/// single-line `line_text`.
fn utf16_col_to_byte_offset(line_text: &str, utf16_col: u32) -> usize {
    let target = utf16_col as usize;
    let mut utf16 = 0;
    for (byte, c) in line_text.char_indices() {
        if utf16 >= target {
            return byte;
        }
        utf16 += c.len_utf16();
    }
    if utf16 <= target {
        return line_text.len();
    }
    line_text.len()
}

/// Convert an LSP [`Position`] into a byte offset into the single-line
/// input `value`. Returns `None` for multi-line ranges, which cannot apply
/// to the single-line Lua prompt.
fn lsp_position_to_offset(value: &str, pos: Position) -> Option<usize> {
    if pos.line != 0 {
        return None;
    }
    Some(utf16_col_to_byte_offset(value, pos.character).min(value.len()))
}

/// Strip snippet placeholders (`$0`, `$1`, `${1:text}`) down to plain text.
///
/// `lua-language-server` marks function completions as snippets, e.g.
/// `print(${1:...})`. Inserting that literally into the single-line REPL
/// would leave placeholder syntax behind, so `${1:foo}` becomes `foo` and
/// bare tabstops disappear.
fn strip_snippet_placeholders(snippet: &str) -> String {
    let mut out = String::with_capacity(snippet.len());
    let mut chars = snippet.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('{') => {
                chars.next();
                let mut num = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() {
                        num.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars.peek() == Some(&':') {
                    chars.next();
                    let mut depth = 1;
                    let mut placeholder = String::new();
                    for ch in chars.by_ref() {
                        if ch == '{' {
                            depth += 1;
                            placeholder.push(ch);
                        } else if ch == '}' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            placeholder.push(ch);
                        } else {
                            placeholder.push(ch);
                        }
                    }
                    out.push_str(&placeholder);
                } else {
                    for ch in chars.by_ref() {
                        if ch == '}' {
                            break;
                        }
                    }
                }
            }
            Some(d) if d.is_ascii_digit() => {
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() {
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
            _ => out.push('$'),
        }
    }
    out
}

/// Text to insert for `item`: `textEdit.new_text`, falling back to
/// `insertText`, then `label`. Snippet-formatted edits are returned as plain
/// text.
fn completion_replacement(item: &CompletionItem) -> String {
    let (raw, is_snippet) = match item.text_edit.as_ref() {
        Some(CompletionTextEdit::Edit(edit)) => (
            edit.new_text.clone(),
            item.insert_text_format == Some(InsertTextFormat::SNIPPET),
        ),
        Some(CompletionTextEdit::InsertAndReplace(edit)) => (
            edit.new_text.clone(),
            item.insert_text_format == Some(InsertTextFormat::SNIPPET),
        ),
        None => match item.insert_text.as_ref() {
            Some(insert) => (
                insert.clone(),
                item.insert_text_format == Some(InsertTextFormat::SNIPPET),
            ),
            None => (item.label.clone(), false),
        },
    };
    if is_snippet {
        strip_snippet_placeholders(&raw)
    } else {
        raw
    }
}

/// Byte range in `value` that accepting `item` should replace.
///
/// Prefers the range from `textEdit` when it targets the single input line;
/// otherwise falls back to the identifier fragment before `cursor`, or to an
/// empty range at `cursor` when there is no fragment. Mirrors
/// `EditorState::insert_completion`'s `fallback_range` handling for the
/// single-line prompt.
fn completion_range(item: &CompletionItem, value: &str, cursor: usize) -> Range<usize> {
    let cursor = cursor.min(value.len());
    let snap = |offset: usize| {
        let offset = offset.min(value.len());
        if value.is_char_boundary(offset) {
            offset
        } else {
            value.floor_char_boundary(offset)
        }
    };
    if let Some(edit) = item.text_edit.as_ref() {
        let lsp_range = match edit {
            CompletionTextEdit::Edit(edit) => Some((edit.range.start, edit.range.end)),
            CompletionTextEdit::InsertAndReplace(edit) => {
                Some((edit.replace.start, edit.replace.end))
            }
        };
        if let Some((start, end)) = lsp_range
            && let (Some(s), Some(e)) = (
                lsp_position_to_offset(value, start),
                lsp_position_to_offset(value, end),
            )
        {
            let (s, e) = (snap(s), snap(e));
            if s <= e {
                return s..e;
            }
        }
    }
    word_start(value, cursor)..cursor
}

#[cfg(test)]
mod tests {
    use super::{
        LuaCompletion, completion_position, completion_range, completion_replacement,
        should_request_completion,
    };
    use async_lsp::lsp_types::{
        CompletionItem, CompletionTextEdit, InsertTextFormat, Position, Range as LspRange, TextEdit,
    };

    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            ..Default::default()
        }
    }

    #[test]
    fn automatic_completion_requires_an_identifier_or_member_access() {
        for value in [
            "pr",
            "local value = pri",
            "string.",
            "py:",
            "make().",
            "make():",
            "_value2",
            "한글",
        ] {
            assert!(should_request_completion(value, value.len()), "{value:?}");
        }
        for value in [
            "",
            "print()",
            "print(tostring(value))",
            "items[1]",
            "{ key = value }",
            "print() ",
            "print(",
            "print(value,",
            "123",
            "value + ",
        ] {
            assert!(!should_request_completion(value, value.len()), "{value:?}");
        }
        // Use the actual caret, including a caret within a function call.
        assert!(should_request_completion("print(va)", 8));
        assert!(!should_request_completion("print() + value", 7));
        assert!(should_request_completion("한글", 4));
    }

    #[test]
    fn closing_a_call_invalidates_pending_completions() {
        let mut completion = LuaCompletion::default();
        let pending = completion.begin_request("print(va", 8);
        // Change clears previous requests before deciding whether to start one.
        completion.clear();
        assert!(!should_request_completion("print(va)", 9));
        assert!(!completion.finish_request(pending, Some(vec![item("value")])));
        assert!(!completion.finish_resolve(pending, 0, item("value")));
        assert!(completion.items.is_none());
    }

    #[test]
    fn suggestions_select_the_servers_preferred_item_immediately() {
        let mut completion = LuaCompletion::default();
        let generation = completion.begin_request("p", 1);
        let mut print = item("print");
        print.sort_text = Some("01".into());
        let mut pairs = item("pairs");
        pairs.sort_text = Some("02".into());
        assert!(completion.finish_request(generation, Some(vec![pairs.clone(), print.clone()])));
        assert_eq!(completion.selected_item("p", 1).unwrap().label, "print");
        pairs.preselect = Some(true);
        let generation = completion.begin_request("p", 1);
        assert!(completion.finish_request(generation, Some(vec![print, pairs])));
        assert_eq!(completion.selected_item("p", 1).unwrap().label, "pairs");
        assert_eq!(completion.item("p", 1, 0).unwrap().label, "print");
        assert!(completion.item("p", 1, 2).is_none());
    }

    #[test]
    fn empty_responses_leave_nothing_to_accept() {
        let mut completion = LuaCompletion::default();
        for items in [None, Some(Vec::new())] {
            let generation = completion.begin_request("p", 1);
            assert!(completion.finish_request(generation, items));
            assert!(completion.items.is_none());
            assert!(completion.selected_item("p", 1).is_none());
        }
    }

    #[test]
    fn insertion_does_not_reopen_suggestions_but_the_next_edit_does() {
        let mut completion = LuaCompletion {
            accepted_input: Some(("string.byte".into(), 11)),
            ..Default::default()
        };
        assert!(completion.take_accepted_input("string.byte", 11));
        assert!(!completion.take_accepted_input("string.byte(", 12));
        completion.accepted_input = Some(("print".into(), 5));
        assert!(!completion.take_accepted_input("print(", 6));
        assert!(!completion.take_accepted_input("print", 5));
    }

    #[test]
    fn resolved_details_cannot_restore_dismissed_or_replaced_suggestions() {
        let mut completion = LuaCompletion::default();
        let old = completion.begin_request("p", 1);
        completion.finish_request(old, Some(vec![item("print")]));
        completion.clear();
        assert!(!completion.finish_resolve(old, 0, item("print")));
        let current = completion.begin_request("pa", 2);
        completion.finish_request(current, Some(vec![item("pairs")]));
        assert!(!completion.finish_resolve(old, 0, item("print")));
        assert!(!completion.finish_resolve(current, 1, item("print")));
        let mut resolved = item("pairs");
        resolved.detail = Some("function pairs(t: table)".into());
        assert!(completion.finish_resolve(current, 0, resolved));
        assert_eq!(
            completion.selected_item("pa", 2).unwrap().detail.as_deref(),
            Some("function pairs(t: table)")
        );
    }

    #[test]
    fn older_responses_cannot_replace_newer_suggestions() {
        let mut completion = LuaCompletion::default();
        let first = completion.begin_request("p", 1);
        let second = completion.begin_request("pr", 2);
        assert!(completion.finish_request(second, Some(vec![item("print")])));
        completion.selected = Some(0);
        assert!(!completion.finish_request(first, Some(vec![item("pairs")])));
        assert_eq!(completion.selected_item("pr", 2).unwrap().label, "print");
        assert!(!completion.finish_request(first, None));
        assert_eq!(completion.selected_item("pr", 2).unwrap().label, "print");
    }

    #[test]
    fn dismissal_and_acceptance_invalidate_pending_responses() {
        let mut completion = LuaCompletion::default();
        let old = completion.begin_request("pr", 2);
        completion.clear();
        assert!(!completion.finish_request(old, Some(vec![item("print")])));
        // Reopening with the same text must still reject the previous request.
        let current = completion.begin_request("pr", 2);
        assert!(!completion.finish_request(old, Some(vec![item("pairs")])));
        assert!(completion.finish_request(current, Some(vec![item("print")])));
        completion.selected = Some(0);
        assert!(completion.selected_item("pr", 1).is_none());
        assert!(completion.selected_item("pa", 2).is_none());
        assert!(completion.selected_item("pr", 2).is_some());
        completion.clear();
        assert!(!completion.finish_request(current, Some(vec![item("print")])));
        assert!(completion.selected_item("pr", 2).is_none());
    }

    #[test]
    fn completion_positions_use_utf16_at_the_actual_cursor() {
        assert_eq!(completion_position("한😀pr", 9), Position::new(0, 5));
        assert_eq!(completion_position("한😀pr", 7), Position::new(0, 3));
        assert_eq!(completion_position("한😀pr", 4), Position::new(0, 1));
        assert_eq!(completion_position("a\n한😀p", 10), Position::new(1, 4));
        assert_eq!(completion_position("", 0), Position::new(0, 0));
    }

    #[test]
    fn accepting_insert_text_replaces_the_identifier_fragment() {
        let item = CompletionItem {
            insert_text: Some("sub".into()),
            ..item("sub")
        };
        let mut value = "string.su(1)".to_string();
        value.replace_range(
            completion_range(&item, &value, 9),
            &completion_replacement(&item),
        );
        assert_eq!(value, "string.sub(1)");
    }

    #[test]
    fn completion_edits_convert_utf16_ranges_to_utf8_bytes() {
        let item = CompletionItem {
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: LspRange::new(Position::new(0, 3), Position::new(0, 5)),
                new_text: "print".into(),
            })),
            ..item("print")
        };
        let mut value = "한😀pr".to_string();
        value.replace_range(
            completion_range(&item, &value, 9),
            &completion_replacement(&item),
        );
        assert_eq!(value, "한😀print");
    }

    #[test]
    fn function_snippets_insert_plain_lua() {
        let item = CompletionItem {
            insert_text: Some("print(${1:...})$0".into()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..item("print")
        };
        let mut value = "pr".to_string();
        value.replace_range(
            completion_range(&item, &value, 2),
            &completion_replacement(&item),
        );
        assert_eq!(value, "print(...)");
    }
}
