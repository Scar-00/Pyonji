//! Single-line Lua input with built-in Lua LSP support.
//!
//! The shared editing engine only offers language-server features on the
//! multi-line [`EditorState`], while [`InputState`] is strictly single-line
//! with no LSP. This module bridges the gap: an [`EditorState`] locked to
//! Lua and forced to a single line, with completion / hover / diagnostics
//! providers backed directly by an embedded `mlua` state. No external
//! `lua-language-server` binary is needed.
//!
//! ```ignore
//! let input = cx.new(|cx| LuaSingleLineInput::new(window, cx));
//! // ... later, in `Render`:
//! div().child(input.clone())
//! ```
//!
//! Ported from the prototype with two additions (`LuaLspStore::new_with_lua`
//! for sharing host state, `SubmitRequested` for host-owned evaluation). The
//! wider method surface is kept for parity and is exercised by the tests
//! below, hence the blanket allow.
#![allow(dead_code)]

use std::{ops::Range, rc::Rc};

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, MouseButton, Render,
    SharedString, Subscription, Task, Window, div, prelude::*,
};
use gpui_component::{
    highlighter::{Diagnostic, DiagnosticSeverity},
    input::{
        CompletionMenuPlacement, CompletionProvider, Editor, EditorState, HoverProvider, InputEvent,
        Position, Rope, RopeExt as _,
    },
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, Documentation,
    Hover, HoverContents, InlineCompletionContext, InlineCompletionResponse, MarkupContent,
    MarkupKind,
};
use mlua::{Lua, MultiValue, Table, Value};

/// Lua keywords offered as completions when no table path precedes the cursor.
const LUA_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
    "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

const MAX_COMPLETIONS: usize = 100;
/// Rows shown in the hand-rolled completion menus (status bar).
pub(crate) const COMPLETION_MENU_LIMIT: usize = 8;
const MAX_TABLE_KEYS: usize = 500;
const MAX_PREVIEW_LEN: usize = 200;
const MAX_RESULT_LEN: usize = 500;

/// One-line documentation for well-known Lua globals, shown in completion
/// details and hover popovers.
fn builtin_doc(qualified: &str) -> Option<&'static str> {
    Some(match qualified {
        "print" => "Prints its arguments to stdout.",
        "pairs" => "Iterates over all key/value pairs of a table.",
        "ipairs" => "Iterates over the array part of a table in order.",
        "tostring" => "Converts its argument to a string.",
        "tonumber" => "Converts its argument to a number, or nil.",
        "type" => "Returns the type name of its argument.",
        "require" => "Loads and caches a module by name.",
        "pcall" => "Calls a function in protected mode, catching errors.",
        "xpcall" => "Like `pcall`, with a custom error handler.",
        "error" => "Raises an error with the given message.",
        "assert" => "Raises an error when its argument is falsy.",
        "select" => "Returns selected arguments (`#` counts them).",
        "next" => "Returns the next key/value pair of a table.",
        "rawget" => "Gets a table field without metamethods.",
        "rawset" => "Sets a table field without metamethods.",
        "rawequal" => "Compares two values without metamethods.",
        "rawlen" => "Length of a value without metamethods.",
        "setmetatable" => "Sets the metatable of a table.",
        "getmetatable" => "Returns the metatable of a value.",
        "load" => "Compiles a chunk from a string without running it.",
        "loadfile" => "Compiles a chunk from a file without running it.",
        "dofile" => "Compiles and runs a Lua file.",
        "collectgarbage" => "Controls the garbage collector.",
        "string" => "Standard string library.",
        "table" => "Standard table library.",
        "math" => "Standard math library.",
        "io" => "Standard I/O library.",
        "os" => "Operating-system facilities.",
        "coroutine" => "Coroutine manipulation.",
        "utf8" => "UTF-8 string support.",
        "debug" => "Debugging facilities.",
        "package" => "Module loading configuration.",
        "_G" => "The global environment table.",
        "_VERSION" => "The Lua version string.",
        "table.insert" => "Inserts an element into a list.",
        "table.remove" => "Removes (and returns) an element from a list.",
        "table.sort" => "Sorts a list in place.",
        "table.concat" => "Joins list elements with a separator.",
        "table.pack" => "Packs arguments into a list with field `n`.",
        "table.unpack" => "Unpacks a list into multiple returns.",
        "string.format" => "Formats a string like C `printf`.",
        "string.sub" => "Returns a substring.",
        "string.find" => "Searches for a pattern, returns its span.",
        "string.gsub" => "Replaces pattern matches.",
        "string.gmatch" => "Iterates over pattern matches.",
        "string.len" => "Returns the byte length of a string.",
        "string.upper" => "Uppercase copy of a string.",
        "string.lower" => "Lowercase copy of a string.",
        "string.rep" => "Repeats a string `n` times.",
        "string.byte" => "Byte values of characters.",
        "string.char" => "String from byte values.",
        "math.floor" => "Largest integer not greater than `x`.",
        "math.ceil" => "Smallest integer not less than `x`.",
        "math.abs" => "Absolute value.",
        "math.sqrt" => "Square root.",
        "math.max" => "Largest of its arguments.",
        "math.min" => "Smallest of its arguments.",
        "math.pi" => "The constant π.",
        "math.random" => "Pseudo-random number.",
        _ => return None,
    })
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Collapse line breaks so a value stays on one line.
///
/// CRLF folds to a single space (not two), keeping pasted code readable.
fn flatten_newlines(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\n', '\r'], " ")
}

fn is_path_char(c: char) -> bool {
    is_ident_char(c) || c == '.' || c == ':'
}

/// Split the text before the cursor into a table path and the unfinished name.
///
/// `"pri"` -> `([], "pri")`, `"table.ins"` -> `(["table"], "ins")`,
/// `"a.b."` -> `(["a", "b"], "")`.
pub fn split_completion_target(before: &str) -> (Vec<String>, String) {
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_path_char(*c))
        .last()
        .map(|(i, _)| i)
        .unwrap_or(before.len());
    let token = before[start..].trim_start_matches(['.', ':']);
    if token.is_empty() {
        return (Vec::new(), String::new());
    }
    // A trailing separator means "list the table's members": the unfinished
    // name is empty and every segment is part of the path.
    if token.ends_with(['.', ':']) {
        let path = token
            .split(['.', ':'])
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        return (path, String::new());
    }
    let mut parts: Vec<String> = token
        .split(['.', ':'])
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let prefix = parts.pop().unwrap_or_default();
    (parts, prefix)
}

/// Expand a dotted Lua expression (`foo.bar:baz`) around a byte offset.
///
/// Returns the byte range and the expression with edge separators trimmed.
pub fn dotted_range_at(line: &str, offset: usize) -> Option<(Range<usize>, String)> {
    if !line.is_char_boundary(offset.min(line.len())) {
        return None;
    }
    let offset = offset.min(line.len());
    let start = line[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_path_char(*c))
        .last()
        .map(|(i, _)| i)
        .unwrap_or(offset);
    let end = line[offset..]
        .char_indices()
        .take_while(|(_, c)| is_path_char(*c))
        .last()
        .map(|(i, c)| offset + i + c.len_utf8())
        .unwrap_or(offset);
    let (mut start, mut end) = (start, end);
    while start < end && matches!(line.as_bytes()[start], b'.' | b':') {
        start += 1;
    }
    while end > start && matches!(line.as_bytes()[end - 1], b'.' | b':') {
        end -= 1;
    }
    if start >= end {
        return None;
    }
    let expr = line[start..end].to_string();
    if !expr.chars().any(is_ident_char) {
        return None;
    }
    Some((start..end, expr))
}

/// Walk a dotted path starting from the Lua globals. Returns the table the
/// path points at, or `None` when any segment is missing or not a table.
fn resolve_table(lua: &Lua, path: &[String]) -> Option<Table> {
    let mut table = lua.globals();
    for segment in path {
        match table.get::<Value>(segment.as_str()).ok()? {
            Value::Table(next) => table = next,
            _ => return None,
        }
    }
    Some(table)
}

/// Resolve a full dotted expression (`a.b.c`, `:` treated like `.`) to a value.
fn value_at(lua: &Lua, expr: &str) -> Option<Value> {
    let mut segments = expr.split(['.', ':']).filter(|s| !s.is_empty());
    let mut value: Value = lua.globals().get(segments.next()?).ok()?;
    for segment in segments {
        let table = match value {
            Value::Table(table) => table,
            _ => return None,
        };
        value = table.get(segment).ok()?;
    }
    Some(value)
}

/// Names of the string keys of `table` starting with `prefix`, alphabetically.
fn matching_keys(table: &Table, prefix: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for pair in table.pairs::<String, Value>() {
        let Ok((key, value)) = pair else {
            continue;
        };
        if key.starts_with(prefix) {
            out.push((key, value));
        }
        if out.len() >= MAX_TABLE_KEYS {
            break;
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn completion_kind(value: &Value) -> CompletionItemKind {
    match value {
        Value::Function(_) => CompletionItemKind::FUNCTION,
        Value::Table(_) => CompletionItemKind::MODULE,
        _ => CompletionItemKind::VARIABLE,
    }
}

fn completion_item(name: String, value: &Value, qualified: &str) -> CompletionItem {
    CompletionItem {
        label: name,
        kind: Some(completion_kind(value)),
        detail: Some(value.type_name().to_string()),
        documentation: builtin_doc(qualified).map(|doc| Documentation::String(doc.to_string())),
        ..Default::default()
    }
}

/// Match labels for a hand-rolled completion menu: the unfinished name's
/// matches, capped for display. Empty when there is no unfinished name, so
/// callers can hide the menu without extra checks. Shared by the status bar,
/// which renders the same menu for Lua and command modes.
pub(crate) fn lua_match_labels(lua: &Lua, before: &str, limit: usize) -> Vec<String> {
    let (path, prefix) = split_completion_target(before);
    // Bare buffer → no menu (would list every global). A trailing dot still
    // lists the table's members.
    if prefix.is_empty() && path.is_empty() {
        return Vec::new();
    }
    lua_completions(lua, before)
        .into_iter()
        .take(limit)
        .map(|item| item.label)
        .collect()
}

/// Build the completion list for the text before the cursor.
pub(crate) fn lua_completions(lua: &Lua, before: &str) -> Vec<CompletionItem> {
    let (path, prefix) = split_completion_target(before);
    let mut items = Vec::new();
    if path.is_empty() {
        let globals = lua.globals();
        for (name, value) in matching_keys(&globals, &prefix) {
            items.push(completion_item(name.clone(), &value, &name));
        }
        for keyword in LUA_KEYWORDS {
            if keyword.starts_with(prefix.as_str()) {
                items.push(CompletionItem {
                    label: keyword.to_string(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    detail: Some("keyword".to_string()),
                    ..Default::default()
                });
            }
        }
    } else if let Some(table) = resolve_table(lua, &path) {
        let base = path.join(".");
        for (name, value) in matching_keys(&table, &prefix) {
            let qualified = format!("{base}.{name}");
            items.push(completion_item(name, &value, &qualified));
        }
    }
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.truncate(MAX_COMPLETIONS);
    items
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn format_value_inner(value: &Value, depth: usize) -> String {
    match value {
        Value::Nil => "nil".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.to_string_lossy(),
        Value::Table(t) => {
            if depth >= 2 {
                return "table".to_string();
            }
            table_preview(t, depth)
        }
        Value::Function(_) => "function".to_string(),
        Value::Thread(_) => "thread".to_string(),
        Value::LightUserData(_) => "lightuserdata".to_string(),
        Value::UserData(_) => "userdata".to_string(),
        Value::Error(e) => format!("error: {e}"),
        Value::Other(_) => "other".to_string(),
    }
}

/// Short preview of a table: `{ 1, 2, 3 }`, `{ ... }`, or `{}`.
fn table_preview(table: &Table, depth: usize) -> String {
    let len = table.len().unwrap_or(0).max(0) as usize;
    if len == 0 {
        let mut pairs = table.pairs::<Value, Value>();
        return match pairs.next() {
            None => "{}".to_string(),
            Some(_) => "{ ... }".to_string(),
        };
    }
    let mut parts = Vec::new();
    for i in 1..=len.min(4) {
        match table.get::<Value>(i as i64) {
            Ok(value) => parts.push(truncate(&format_value_inner(&value, depth + 1), 40)),
            Err(_) => parts.push("?".to_string()),
        }
    }
    let mut preview = format!("{{ {} }}", parts.join(", "));
    if len > 4 {
        preview.push_str(&format!(" (+{} more)", len - 4));
    }
    preview
}

/// Format the values of an evaluated chunk the way a REPL prints them.
fn format_multi(values: &MultiValue) -> String {
    let text = values
        .iter()
        .map(|value| format_value_inner(value, 0))
        .collect::<Vec<_>>()
        .join("\t");
    truncate(&text, MAX_RESULT_LEN)
}

/// Evaluate one line of Lua, expression-first like a REPL.
///
/// `Chunk::eval` already tries `return <code>` before running the chunk as a
/// block, so expressions yield their values and statements run for effect
/// (producing an empty result).
pub fn eval_line(lua: &Lua, line: &str) -> Result<String, String> {
    let code = line.trim();
    if code.is_empty() {
        return Ok(String::new());
    }
    match lua.load(code).set_name("input").eval::<MultiValue>() {
        Ok(values) => Ok(format_multi(&values)),
        Err(err) => Err(err.to_string()),
    }
}

/// Extract the offending token from a Lua syntax error, if it names one.
///
/// `input:1: syntax error near 'foo'` -> `Some("foo")`, `... near <eof>` ->
/// `None`.
fn error_token(message: &str) -> Option<&str> {
    let rest = message.split("near ").nth(1)?;
    let rest = rest.trim_start();
    if let Some(token) = rest.strip_prefix('\'') {
        return token.split('\'').next().filter(|t| !t.is_empty());
    }
    None
}

/// Byte range to underline for a syntax error: the offending token's last
/// occurrence, falling back to the whole line.
fn error_range(text: &Rope, src: &str, message: &str) -> (Position, Position) {
    let whole = || {
        (
            Position::new(0, 0),
            text.offset_to_position(text.len()),
        )
    };
    let Some(token) = error_token(message) else {
        return whole();
    };
    let Some(start) = src.rfind(token) else {
        return whole();
    };
    let end = start + token.len();
    if !src.is_char_boundary(start) || !src.is_char_boundary(end) {
        return whole();
    }
    (text.offset_to_position(start), text.offset_to_position(end))
}

/// Syntax-check single-line Lua without running it.
///
/// Compiles via `into_function` (no execution) and reports failures as an
/// [`Diagnostic`] over the offending token, or the whole line.
pub fn check_syntax(lua: &Lua, text: &Rope) -> Option<Diagnostic> {
    let src = text.to_string();
    if src.trim().is_empty() {
        return None;
    }
    match lua.load(&src).set_name("input").into_function() {
        Ok(_) => None,
        Err(err) => {
            let message = err.to_string();
            let (start, end) = error_range(text, &src, &message);
            Some(
                Diagnostic::new(start..end, message)
                    .with_severity(DiagnosticSeverity::Error)
                    .with_source("lua")
                    .with_code("syntax-error"),
            )
        }
    }
}

/// Build the hover card for the expression under `offset`, if it resolves to
/// a Lua value or a known keyword / builtin.
fn lua_hover(lua: &Lua, text: &Rope, offset: usize) -> Option<Hover> {
    let src = text.to_string();
    let (range, expr) = dotted_range_at(&src, offset)?;
    let markdown = if let Some(value) = value_at(lua, &expr) {
        let ty = value.type_name();
        let mut md = format!("```lua\n{ty} {expr}\n```");
        let preview = truncate(&format_value_inner(&value, 1), MAX_PREVIEW_LEN);
        if preview != ty {
            md.push_str(&format!("\n\n{preview}"));
        }
        let last = expr.rsplit(['.', ':']).next().unwrap_or(&expr);
        if let Some(doc) = builtin_doc(&expr).or_else(|| builtin_doc(last)) {
            md.push_str(&format!("\n\n---\n{doc}"));
        }
        md
    } else if LUA_KEYWORDS.contains(&expr.as_str()) {
        format!("Lua keyword `{expr}`")
    } else {
        builtin_doc(&expr)?.to_string()
    };
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(lsp_types::Range {
            start: text.offset_to_position(range.start),
            end: text.offset_to_position(range.end),
        }),
    })
}

/// mlua-backed language support for the single-line Lua input.
///
/// Held in an `Rc` and shared between the [`LuaSingleLineInput`] view (for
/// diagnostics and evaluation) and the editor's LSP slots (for completion
/// and hover). Everything runs synchronously on the UI thread against the
/// embedded Lua state, so there is no server process to manage.
pub struct LuaLspStore {
    lua: Lua,
}

impl LuaLspStore {
    pub fn new() -> Self {
        Self::new_with_lua(Lua::new())
    }

    /// Share an existing Lua state (clones are cheap handles to the same
    /// state), so completions, hover, and evaluation see the host's globals —
    /// including anything the user assigned from earlier evaluations.
    pub fn new_with_lua(lua: Lua) -> Self {
        Self { lua }
    }

    /// The embedded Lua state. Register host globals here to make them
    /// visible to completion, hover, and evaluation:
    ///
    /// ```ignore
    /// input.lua().globals().set("answer", 42)?;
    /// ```
    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Evaluate one line of Lua in the shared state. Assignments persist,
    /// so later completions and hovers see the new globals.
    pub fn eval(&self, line: &str) -> Result<String, String> {
        eval_line(&self.lua, line)
    }

    /// Syntax-check `text` without running it.
    pub fn diagnostic(&self, text: &Rope) -> Option<Diagnostic> {
        check_syntax(&self.lua, text)
    }
}

impl Default for LuaLspStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CompletionProvider for LuaLspStore {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let offset = offset.min(text.len());
        let before = text.slice(..offset).to_string();
        let (_, prefix) = split_completion_target(&before);
        let prefix_start = offset - prefix.len();
        let mut items = lua_completions(&self.lua, &before);
        // Replace only the unfinished name so `table.ins` completes to
        // `table.insert` instead of clobbering the table path.
        let range = lsp_types::Range {
            start: text.offset_to_position(prefix_start),
            end: text.offset_to_position(offset),
        };
        for item in &mut items {
            item.text_edit = Some(lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit {
                range,
                new_text: item.label.clone(),
            }));
        }
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn inline_completion(
        &self,
        _rope: &Rope,
        _offset: usize,
        _trigger: InlineCompletionContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<anyhow::Result<InlineCompletionResponse>> {
        Task::ready(Ok(InlineCompletionResponse::Array(vec![])))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        !new_text.is_empty()
            && new_text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':'))
    }
}

impl HoverProvider for LuaLspStore {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<anyhow::Result<Option<Hover>>> {
        Task::ready(Ok(lua_hover(&self.lua, text, offset)))
    }
}

/// Result of pressing Enter in a [`LuaSingleLineInput`].
#[derive(Debug, Clone)]
pub struct LuaSubmit {
    /// The submitted line.
    pub code: SharedString,
    /// Printed evaluation result, or the error message when `is_error`.
    pub result: SharedString,
    pub is_error: bool,
}

/// Events emitted by [`LuaSingleLineInput`].
#[derive(Debug, Clone)]
pub enum LuaInputEvent {
    /// The text changed (already stripped of newlines).
    Change,
    /// Enter was pressed: the line was evaluated and the field cleared.
    Submit(LuaSubmit),
    /// Enter was pressed with `evaluate_on_enter(false)`: the host owns
    /// evaluation (e.g. to inject its own environment) and must clear the
    /// field itself.
    SubmitRequested(SharedString),
}

/// A single-line text input for Lua with built-in LSP support.
///
/// Backed by an [`EditorState`] configured for Lua (`language("lua")`,
/// `submit_on_enter(true)`, no line numbers / folding / indent guides /
/// wrapping / search) so it gets tree-sitter highlighting plus the
/// completion, hover, and diagnostics of [`LuaLspStore`]. Newlines can never
/// survive: Enter submits instead of inserting one, and pasted line breaks
/// are flattened to spaces on change.
pub struct LuaSingleLineInput {
    state: Entity<EditorState>,
    lsp: Rc<LuaLspStore>,
    evaluate_on_enter: bool,
    _subscriptions: Vec<Subscription>,
}

impl LuaSingleLineInput {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(Lua::new(), true, window, cx)
    }

    /// Share the host's Lua state (for runtime-aware completion) and leave
    /// Enter evaluation to the host via [`LuaInputEvent::SubmitRequested`].
    /// The host evaluates (with whatever environment its semantics need),
    /// records history, and clears the field itself.
    pub fn new_with_lua(lua: Lua, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::build(lua, false, window, cx)
    }

    fn build(
        lua: Lua,
        evaluate_on_enter: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let lsp = Rc::new(LuaLspStore::new_with_lua(lua));
        let completion: Rc<dyn CompletionProvider> = lsp.clone();
        let hover: Rc<dyn HoverProvider> = lsp.clone();
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("lua")
                .line_number(false)
                .folding(false)
                .indent_guides(false)
                .soft_wrap(false)
                .searchable(false)
                .submit_on_enter(true)
                .placeholder("Lua expression — Enter to run")
        });
        state.update(cx, |state, cx| {
            state.lsp_mut().completion_provider = Some(completion);
            state.lsp_mut().hover_provider = Some(hover);
            // The bar is docked at the bottom of the window: open the
            // completion list above the input so it is not clipped.
            state.lsp_mut().completion_menu.placement = CompletionMenuPlacement::Above;
            cx.notify();
        });
        let _subscriptions = vec![cx.subscribe_in(&state, window, Self::on_editor_event)];
        Self {
            state,
            lsp,
            evaluate_on_enter,
            _subscriptions,
        }
    }

    /// Whether Enter evaluates inside the input (`true`) or defers to the
    /// host via [`LuaInputEvent::SubmitRequested`] (`false`).
    pub fn evaluate_on_enter(&self) -> bool {
        self.evaluate_on_enter
    }

    fn on_editor_event(
        &mut self,
        state: &Entity<EditorState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                // `set_value` below does not re-emit, so this cannot recurse.
                let text = state.read(cx).text().to_string();
                if text.contains(['\n', '\r']) {
                    let cleaned: SharedString = flatten_newlines(&text).into();
                    state.update(cx, |state, cx| state.set_value(cleaned, window, cx));
                }
                let diagnostic = state.read(cx).text().clone();
                let diagnostic = check_syntax(&self.lsp.lua, &diagnostic);
                state.update(cx, |state, cx| {
                    if let Some(set) = state.diagnostics_mut() {
                        set.clear();
                        if let Some(diagnostic) = diagnostic {
                            set.extend([diagnostic]);
                        }
                    }
                    cx.notify();
                });
                cx.emit(LuaInputEvent::Change);
            }
            InputEvent::PressEnter { shift: false, .. } => {
                let code = state.read(cx).value().to_string();
                let code = code.trim().to_string();
                if code.is_empty() {
                    return;
                }
                if !self.evaluate_on_enter {
                    cx.emit(LuaInputEvent::SubmitRequested(code.into()));
                    return;
                }
                let (result, is_error) = match self.lsp.eval(&code) {
                    Ok(value) => (value, false),
                    Err(err) => (err, true),
                };
                state.update(cx, |state, cx| state.set_value("", window, cx));
                cx.emit(LuaInputEvent::Submit(LuaSubmit {
                    code: code.into(),
                    result: result.into(),
                    is_error,
                }));
            }
            _ => {}
        }
    }

    /// The underlying editor state, for advanced use (selection, focus, …).
    pub fn editor(&self) -> &Entity<EditorState> {
        &self.state
    }

    /// The shared Lua language support: completions, hover, diagnostics, eval.
    pub fn lsp(&self) -> &Rc<LuaLspStore> {
        &self.lsp
    }

    /// Direct access to the embedded Lua state for registering host globals.
    pub fn lua(&self) -> &Lua {
        &self.lsp.lua
    }

    /// Current single-line value.
    pub fn value(&self, cx: &App) -> SharedString {
        self.state.read(cx).value()
    }

    /// Replace the value (newlines are flattened to spaces).
    pub fn set_value(&self, value: &str, window: &mut Window, cx: &mut App) {
        let cleaned: SharedString = flatten_newlines(value).into();
        self.state.update(cx, |state, cx| {
            state.set_value(cleaned, window, cx);
        });
    }

    /// Evaluate a line of Lua in the shared state.
    pub fn eval(&self, line: &str) -> Result<String, String> {
        self.lsp.eval(line)
    }

    /// Move keyboard focus into the input.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Drop the editor's built-in completion popover. Hosts that render
    /// their own menu (like the status bar, which shares one menu design
    /// across Lua and command modes) call this once; hover, diagnostics,
    /// and evaluation are unaffected.
    pub fn disable_builtin_completion(&self, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.lsp_mut().completion_provider = None;
            cx.notify();
        });
    }

    /// Whether the input currently holds keyboard focus.
    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.state.read(cx).focus_handle(cx).is_focused(window)
    }
}

impl EventEmitter<LuaInputEvent> for LuaSingleLineInput {}

impl Focusable for LuaSingleLineInput {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx)
    }
}

impl Render for LuaSingleLineInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.is_focused(window, cx);
        div()
            .w_full()
            .rounded(crate::theme::radius::SM)
            .bg(crate::theme::role::input_bg())
            .border_1()
            .border_color(if focused {
                crate::theme::role::input_border_focus()
            } else {
                crate::theme::role::input_border()
            })
            // Clicking anywhere in the frame focuses the text, like `Input`.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.focus(window, cx);
                cx.notify();
            }))
            .child(Editor::new(&self.state).px_1().text_sm().appearance(false).bordered(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn single_line_enforces_newlines_and_submits(cx: &mut gpui::TestAppContext) {
        use gpui_component::input::Enter;
        use std::{cell::RefCell, rc::Rc};

        cx.update(gpui_component::init);
        let submitted: Rc<RefCell<Vec<LuaSubmit>>> = Rc::new(RefCell::new(Vec::new()));
        let (view, vcx) = cx.add_window_view(LuaSingleLineInput::new);

        // LSP providers are wired at construction.
        vcx.update(|_, cx| {
            let editor = view.read(cx).editor().clone();
            let state = editor.read(cx);
            assert!(
                state.lsp().completion_provider.is_some(),
                "completion provider must be built in"
            );
            assert!(
                state.lsp().hover_provider.is_some(),
                "hover provider must be built in"
            );
            assert_eq!(
                state.lsp().completion_menu.placement,
                CompletionMenuPlacement::Above,
                "the bottom-docked bar must open completions above the input"
            );
        });

        let _sub = vcx.update(|_, cx| {
            cx.subscribe(&view, {
                let submitted = submitted.clone();
                move |_, event: &LuaInputEvent, _| {
                    if let LuaInputEvent::Submit(submit) = event {
                        submitted.borrow_mut().push(submit.clone());
                    }
                }
            })
        });

        vcx.update(|window, cx| {
            let _ = window.draw(cx);
            view.update(cx, |view, cx| view.focus(window, cx));
        });
        vcx.run_until_parked();

        // Pasted line breaks are flattened through the Change path.
        vcx.update(|window, cx| {
            let editor = view.read(cx).editor().clone();
            editor.update(cx, |state, cx| {
                state.replace_all("a\nb\r\nc", window, cx);
            });
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(view.read(cx).value(cx).as_ref(), "a b c");
        });

        // Enter evaluates the line, reports the result, and clears the field.
        vcx.update(|window, cx| {
            view.update(cx, |input, cx| input.set_value("1 + 2", window, cx));
        });
        vcx.update(|_, cx| {
            assert_eq!(view.read(cx).value(cx).as_ref(), "1 + 2");
        });
        vcx.dispatch_action(Enter {
            secondary: false,
            shift: false,
        });
        vcx.run_until_parked();
        assert_eq!(submitted.borrow().len(), 1);
        assert_eq!(submitted.borrow()[0].code.as_ref(), "1 + 2");
        assert_eq!(submitted.borrow()[0].result.as_ref(), "3");
        assert!(!submitted.borrow()[0].is_error);
        vcx.update(|_, cx| {
            assert_eq!(view.read(cx).value(cx).as_ref(), "");
        });

        // Runtime errors are reported, not raised.
        vcx.update(|window, cx| {
            view.update(cx, |input, cx| {
                input.set_value("nosuchfn_xyz()", window, cx);
            });
        });
        vcx.dispatch_action(Enter {
            secondary: false,
            shift: false,
        });
        vcx.run_until_parked();
        assert_eq!(submitted.borrow().len(), 2);
        assert!(submitted.borrow()[1].is_error);
    }

    #[test]
    fn completion_target_splits_path_and_prefix() {
        assert_eq!(split_completion_target(""), (vec![], "".to_string()));
        assert_eq!(
            split_completion_target("pri"),
            (vec![], "pri".to_string())
        );
        assert_eq!(
            split_completion_target("table.ins"),
            (vec!["table".to_string()], "ins".to_string())
        );
        assert_eq!(
            split_completion_target("a.b."),
            (vec!["a".to_string(), "b".to_string()], "".to_string())
        );
        assert_eq!(
            split_completion_target("x = table:ins"),
            (vec!["table".to_string()], "ins".to_string())
        );
        // Only the trailing token counts.
        assert_eq!(
            split_completion_target("f(1, sta"),
            (vec![], "sta".to_string())
        );
    }

    #[test]
    fn dotted_range_expands_both_directions() {
        let line = "x = table.insert(t, 1)";
        let offset = line.find("insert").unwrap() + 2;
        let (range, expr) = dotted_range_at(line, offset).unwrap();
        assert_eq!(expr, "table.insert");
        assert_eq!(&line[range], "table.insert");

        assert!(dotted_range_at("   ", 1).is_none());
        assert!(dotted_range_at("", 0).is_none());
        // Edge separators are trimmed.
        let (range, expr) = dotted_range_at("a.", 2).unwrap();
        assert_eq!(expr, "a");
        assert_eq!(range, 0..1);
    }

    #[test]
    fn eval_handles_expressions_statements_and_errors() {
        let lua = Lua::new();
        assert_eq!(eval_line(&lua, "1 + 2").unwrap(), "3");
        assert_eq!(eval_line(&lua, "\"hi\"").unwrap(), "hi");
        assert_eq!(eval_line(&lua, "1, 2").unwrap(), "1\t2");
        assert_eq!(eval_line(&lua, "  ").unwrap(), "");
        // Statements run for effect and persist.
        assert_eq!(eval_line(&lua, "answer = 40 + 2").unwrap(), "");
        assert_eq!(eval_line(&lua, "answer").unwrap(), "42");
        assert!(eval_line(&lua, "1 +").is_err());
        assert!(eval_line(&lua, "nosuchfn()").is_err());
    }

    #[test]
    fn eval_formats_tables() {
        let lua = Lua::new();
        assert_eq!(eval_line(&lua, "{}").unwrap(), "{}");
        assert_eq!(eval_line(&lua, "{1, 2, 3}").unwrap(), "{ 1, 2, 3 }");
    }

    #[test]
    fn syntax_check_accepts_valid_and_flags_broken() {
        let lua = Lua::new();
        assert!(check_syntax(&lua, &Rope::from("print(1)")).is_none());
        assert!(check_syntax(&lua, &Rope::from("")).is_none());
        assert!(check_syntax(&lua, &Rope::from("   ")).is_none());

        let diag = check_syntax(&lua, &Rope::from("local x = ")).unwrap();
        assert_eq!(diag.severity, DiagnosticSeverity::Error);
        assert!(!diag.message.is_empty());

        // The offending token is underlined when the message names one.
        let text = Rope::from("print((1)");
        let diag = check_syntax(&lua, &text).unwrap();
        let start = text.position_to_offset(&diag.range.start);
        let end = text.position_to_offset(&diag.range.end);
        assert!(end > start);
        assert!(end <= text.len());
    }

    #[test]
    fn match_labels_gate_and_cap_for_menus() {
        let lua = Lua::new();
        // No unfinished name → no menu.
        assert!(lua_match_labels(&lua, "", 8).is_empty());
        // A trailing dot lists the table's members.
        assert!(!lua_match_labels(&lua, "table.", 8).is_empty());
        // Otherwise capped label list, best names included.
        let labels = lua_match_labels(&lua, "pri", 8);
        assert!(labels.contains(&"print".to_string()));
        assert!(labels.len() <= 8);
        let labels = lua_match_labels(&lua, "table.ins", 8);
        assert!(labels.contains(&"insert".to_string()));
    }

    #[test]
    fn completions_cover_globals_members_and_keywords() {
        let lua = Lua::new();
        let labels = |items: Vec<CompletionItem>| {
            items.into_iter().map(|i| i.label).collect::<Vec<_>>()
        };

        let items = lua_completions(&lua, "pri");
        assert!(labels(items).contains(&"print".to_string()));

        let items = lua_completions(&lua, "table.ins");
        assert!(labels(items).contains(&"insert".to_string()));

        let items = lua_completions(&lua, "table.");
        assert!(items.len() > 3);

        let items = lua_completions(&lua, "loc");
        assert!(labels(items).contains(&"local".to_string()));

        // User-defined globals become completable after eval.
        eval_line(&lua, "mydemo_global_xyz = 1").unwrap();
        let items = lua_completions(&lua, "mydemo_g");
        assert!(labels(items).contains(&"mydemo_global_xyz".to_string()));

        // Unknown table paths complete to nothing.
        assert!(lua_completions(&lua, "nosuchtable_xyz.").is_empty());
    }

    #[test]
    fn hover_resolves_values_and_keywords() {
        let lua = Lua::new();
        let text = Rope::from("print(1)");
        let hover = lua_hover(&lua, &text, 2).unwrap();
        match hover.contents {
            HoverContents::Markup(md) => assert!(md.value.contains("print")),
            _ => panic!("expected markdown hover"),
        }

        let text = Rope::from("local x = 1");
        assert!(lua_hover(&lua, &text, 1).is_some());

        let text = Rope::from("   ");
        assert!(lua_hover(&lua, &text, 1).is_none());
    }

    #[test]
    fn error_token_parses_lua_messages() {
        assert_eq!(
            error_token("input:1: syntax error near 'foo'"),
            Some("foo")
        );
        assert_eq!(error_token("input:1: 'end' expected near <eof>"), None);
        assert_eq!(error_token("some other message"), None);
    }

    #[test]
    fn newlines_flatten_to_single_spaces() {
        assert_eq!(flatten_newlines("a\nb"), "a b");
        assert_eq!(flatten_newlines("a\r\nb"), "a b");
        assert_eq!(flatten_newlines("a\rb"), "a b");
        assert_eq!(flatten_newlines("no breaks"), "no breaks");
    }

    #[test]
    fn store_exposes_lua_and_diagnostics() {
        let store = LuaLspStore::new();
        store
            .lua()
            .globals()
            .set("store_probe_xyz", 7)
            .unwrap();
        let items = lua_completions(store.lua(), "store_pro");
        assert!(
            items.iter().any(|i| i.label == "store_probe_xyz"),
            "host-registered globals must be completable"
        );
        assert!(
            store
                .diagnostic(&Rope::from("store_probe_xyz +"))
                .is_some()
        );
        assert!(store.diagnostic(&Rope::from("return 1")).is_none());
    }
}
