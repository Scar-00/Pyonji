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
use std::ops::Range;

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
    }

    pub fn clear_completion(&mut self, cx: &mut Context<Self>) {
        self.items = None;
        self.selected = None;
        cx.notify();
    }

    fn spawn_lsp(cx: &mut Context<Self>) {
        cx.spawn(async |this, cx| {
            let path = "~/.local/share/nvim/mason/bin/lua-language-server";
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
                    let view = LuaView::new(
                        &self.pyonji,
                        self.items.clone(),
                        self.selected,
                        self.scroll_handle.clone(),
                    )
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
                        this.scroll_handle
                            .scroll_to_item(next, ScrollStrategy::Nearest);
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
                        this.scroll_handle
                            .scroll_to_item(prev, ScrollStrategy::Nearest);
                        this.selected = Some(prev);
                        cx.notify();
                    }));
                    this.child(view)
                }
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
                                let items = LspClient::get_completions_for(
                                    server,
                                    uri,
                                    trigger,
                                    index as u32,
                                )
                                .compat()
                                .await?;
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

        let edit = self.selected.and_then(|selected| {
            self.items
                .as_ref()
                .and_then(|items| items.get(selected).cloned())
        });

        let selected = self.selected;
        let py_accept = self.pyonji.clone();

        v_flex()
            .key_context(StatusBar::CONTEXT)
            .when_some(self.on_next, |this, a| this.on_action(a))
            .when_some(self.on_prev, |this, a| this.on_action(a))
            .on_action(window.listener_for(
                &input_state,
                move |input, _: &LuaAcceptCompletion, window, cx| {
                    let Some(item) = edit.clone() else {
                        return;
                    };
                    let value = input.value().to_string();
                    let cursor = input.cursor().min(value.len());
                    let range = completion_range(&item, &value, cursor);
                    let new_text = completion_replacement(&item);
                    input.set_selected_range(range, cx);
                    input.replace(new_text, window, cx);
                    if let Some(py) = py_accept.upgrade() {
                        let status_bar = cx.read_entity(&py, |py, _| py.status_bar.clone());
                        status_bar.update(cx, |bar, cx| {
                            bar.clear_completion(cx);
                        });
                    }
                },
            ))
            .w_full()
            .children(
                self.items
                    .map(|items| CompletionMenu::new(items, selected, self.scroll_handle)),
            )
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
    fn new(
        items: Vec<CompletionItem>,
        selected: Option<usize>,
        scroll_handle: UniformListScrollHandle,
    ) -> Self {
        Self {
            items,
            selected,
            scroll_handle,
        }
    }

    fn render_item(item: &CompletionItem, selected: bool, cx: &mut App) -> Div {
        let theme = cx.theme();
        let kind = item.kind.map(|k| format!("{k:?}")).unwrap_or_default();
        h_flex().w_full().child(
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
                    } else {
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
                ),
        )
    }
}

impl RenderOnce for CompletionMenu {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let Self {
            items,
            selected,
            scroll_handle,
        } = self;
        deferred(
            anchored()
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
                                move |range, _, cx| {
                                    let start = range.start;

                                    items[range]
                                        .iter()
                                        .enumerate()
                                        .map(|(i, item)| {
                                            let i = i + start;
                                            Self::render_item(item, Some(i) == selected, cx)
                                        })
                                        .collect()
                                },
                            )
                            .track_scroll(&scroll_handle)
                            .with_sizing_behavior(ListSizingBehavior::Infer)
                            .with_horizontal_sizing_behavior(
                                ListHorizontalSizingBehavior::Unconstrained,
                            )
                            .h_full(),
                        ),
                ),
        )
        .priority_auto()
    }
}

/// Whether `c` continues a Lua identifier (`foo`, `bar2`, `_x`).
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
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
    if item.insert_text.is_some() && item.text_edit.is_none() {
        return cursor..cursor;
    }
    word_start(value, cursor)..cursor
}
