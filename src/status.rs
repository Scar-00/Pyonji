use std::time::{Duration, Instant};

use mlua::prelude::*;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::renderer::StatusInput;
use crate::{config, App};

const MESSAGE_TIMEOUT: Duration = Duration::from_secs(5);

pub enum Mode {
    Command,
    Rename,
    Lua,
}

pub struct StatusBar {
    prompt: Option<Prompt>,
    message: Option<(Instant, String)>,
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            prompt: None,
            message: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.prompt.is_some()
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_ref().map(|(_, text)| text.as_str())
    }

    pub fn show_message(&mut self, text: String) {
        self.message = Some((Instant::now(), text));
    }

    pub fn message_expiry(&self) -> Option<Instant> {
        self.message.as_ref().map(|(shown_at, _)| *shown_at + MESSAGE_TIMEOUT)
    }

    pub fn clear_message_if_expired(&mut self) -> bool {
        let expired = self
            .message
            .as_ref()
            .is_some_and(|(shown_at, _)| Instant::now() >= *shown_at + MESSAGE_TIMEOUT);
        if expired {
            self.message = None;
        }
        expired
    }

    pub fn open(&mut self, mode: Mode) {
        self.message = None;
        self.prompt = Some(Prompt::new(mode));
    }

    pub fn open_rename(&mut self, name: String) {
        self.message = None;
        let mut prompt = Prompt::new(Mode::Rename);
        if !name.is_empty() {
            prompt.insert(&name);
        }
        self.prompt = Some(prompt);
    }

    pub fn status_input(&self, cols: u16) -> Option<StatusInput> {
        self.prompt.as_ref().map(|prompt| prompt.display(cols))
    }

    pub fn handle_key(&mut self, app: &mut App, event: &KeyEvent) {
        let Some(mut prompt) = self.prompt.take() else {
            return;
        };
        let PhysicalKey::Code(code) = event.physical_key else {
            self.prompt = Some(prompt);
            return;
        };
        if event.state != ElementState::Pressed {
            self.prompt = Some(prompt);
            return;
        }
        let mut keep = true;
        match code {
            KeyCode::Escape => keep = false,
            KeyCode::Enter => {
                let input = prompt.buffer.trim().to_string();
                keep = false;
                if !input.is_empty() {
                    match prompt.mode {
                        Mode::Command => {
                            prompt.push_history(input.clone());
                            self.execute_command(app, &input);
                        }
                        Mode::Rename => {
                            app.rename_active(&input);
                        }
                        Mode::Lua => {
                            prompt.push_history(input.clone());
                            self.evaluate_lua(app, &input);
                        }
                    }
                }
            }
            KeyCode::Backspace => prompt.delete_before(),
            KeyCode::Delete => prompt.delete_at(),
            KeyCode::ArrowLeft => prompt.cursor_left(),
            KeyCode::ArrowRight => prompt.cursor_right(),
            KeyCode::Home => prompt.cursor = 0,
            KeyCode::End => prompt.cursor = prompt.buffer.len(),
            KeyCode::ArrowUp => prompt.history_prev(),
            KeyCode::ArrowDown => prompt.history_next(),
            _ => {
                if let Some(text) = &event.text
                    && !text.is_empty()
                {
                    prompt.insert(text);
                }
            }
        }
        if keep {
            self.prompt = Some(prompt);
        }
    }

    fn execute_command(&mut self, app: &mut App, input: &str) {
        let mut overlay = app.overlay.take();
        let handled = match overlay.as_mut() {
            Some(overlay) => overlay.execute_command(app, input),
            None => false,
        };
        app.overlay = overlay;
        if !handled {
            self.message = Some((
                Instant::now(),
                format!(
                    "unknown command: {}",
                    input.split(' ').next().unwrap_or(input)
                ),
            ));
        }
    }

    fn evaluate_lua(&mut self, app: &mut App, expr: &str) {
        let lua = app.lua.clone();
        let out = config::with_env(app, |_| {
            let value = lua.load(expr).eval::<LuaValue>()?;
            let res = Self::format_lua_value(&lua, value, 0)?;
            Ok(res)
        });
        let message = match out {
            Err(error) => format!("{error}"),
            Ok(message) => message,
        };
        self.message = Some((Instant::now(), message.replace(['\n', '\r'], " ")));
    }

    fn format_lua_value(lua: &Lua, value: LuaValue, depth: usize) -> LuaResult<String> {
        Ok(match value {
            LuaValue::Nil => "nil".to_string(),
            LuaValue::Boolean(b) => b.to_string(),
            LuaValue::Integer(i) => i.to_string(),
            LuaValue::Number(n) => n.to_string(),
            LuaValue::String(s) => s.to_str()?.to_string(),
            LuaValue::Table(t) if depth < 3 => {
                let mut parts = Vec::new();
                for pair in t.pairs::<LuaValue, LuaValue>() {
                    let (key, value) = pair?;
                    let key = Self::format_lua_value(lua, key, depth + 1)?;
                    let value = Self::format_lua_value(lua, value, depth + 1)?;
                    parts.push(format!("{key} = {value}"));
                    if parts.len() >= 6 {
                        parts.push("...".to_string());
                        break;
                    }
                }
                format!("{{ {} }}", parts.join(", "))
            }
            value => {
                let tostring: LuaFunction = lua.globals().get("tostring")?;
                tostring.call(value)?
            }
        })
    }
}

struct Prompt {
    mode: Mode,
    buffer: String,
    cursor: usize,
    history: Vec<String>,
    history_index: Option<usize>,
}

impl Prompt {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            buffer: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_index: None,
        }
    }

    fn insert(&mut self, text: &str) {
        self.buffer.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn delete_before(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let index = self.buffer[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        self.buffer.remove(index);
        self.cursor = index;
    }

    fn delete_at(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        self.buffer.remove(self.cursor);
    }

    fn cursor_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor = self.buffer[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
    }

    fn cursor_right(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        self.cursor = self.buffer[self.cursor..]
            .char_indices()
            .nth(1)
            .map_or(self.buffer.len(), |(index, _)| self.cursor + index);
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => self.history_index = Some(self.history.len() - 1),
            Some(index) if index > 0 => self.history_index = Some(index - 1),
            _ => return,
        }
        let index = self.history_index.expect("just set");
        self.buffer = self.history[index].clone();
        self.cursor = self.buffer.len();
    }

    fn history_next(&mut self) {
        match self.history_index {
            Some(index) if index + 1 < self.history.len() => {
                self.history_index = Some(index + 1);
                self.buffer = self.history[index + 1].clone();
            }
            Some(_) => {
                self.history_index = None;
                self.buffer.clear();
            }
            None => return,
        }
        self.cursor = self.buffer.len();
    }

    fn push_history(&mut self, entry: String) {
        if self.history.last() != Some(&entry) {
            self.history.push(entry);
        }
        self.history_index = None;
    }

    fn display(&self, cols: u16) -> StatusInput {
        let left_limit = usize::from(cols.max(1));

        let prompt_text = match self.mode {
            Mode::Command => " : ",
            Mode::Rename => " R: ",
            Mode::Lua => " > ",
        };
        let prompt_cols = prompt_text.chars().count();

        let cursor_col = self.buffer[..self.cursor.min(self.buffer.len())]
            .chars()
            .count();
        let max_cols = left_limit.saturating_sub(prompt_cols).max(1);

        let start = if cursor_col >= max_cols {
            cursor_col - max_cols + 1
        } else {
            0
        };
        let visible = self.buffer.chars().skip(start).take(max_cols).collect::<String>();
        let visible_cursor = cursor_col.saturating_sub(start);

        StatusInput {
            prompt: prompt_text.to_string(),
            text: visible,
            cursor_col: prompt_cols + visible_cursor,
        }
    }
}
