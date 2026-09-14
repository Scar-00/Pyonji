use std::time::{Duration, Instant};

use gpui::{App, Entity, KeyDownEvent};
use mlua::prelude::*;

use crate::{Surface, config, overlay};

pub const MESSAGE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Command,
    Rename,
    Lua,
}

impl Mode {
    pub fn prompt_text(self) -> &'static str {
        match self {
            Mode::Command => " : ",
            Mode::Rename => " R: ",
            Mode::Lua => " > ",
        }
    }
}

pub struct StatusBar {
    prompt: Option<Prompt>,
    message: Option<(Instant, String)>,
    command_history: Vec<String>,
    lua_history: Vec<String>,
    lua_history_index: Option<usize>,
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
            command_history: Vec::new(),
            lua_history: Vec::new(),
            lua_history_index: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.prompt.is_some()
    }

    /// Mode of the active prompt, if any.
    pub fn prompt_mode(&self) -> Option<Mode> {
        self.prompt.as_ref().map(|prompt| prompt.mode)
    }

    /// Close the prompt without submitting.
    pub fn cancel_prompt(&mut self) {
        self.prompt = None;
    }

    /// Highlighted completion row for the menu (meaningful in command mode).
    pub fn completion_selected(&self) -> usize {
        self.prompt.as_ref().map_or(0, |prompt| prompt.completion_index)
    }

    /// Match count for the command menu, so Up/Down can decide between
    /// navigating it and walking history. Takes the active prompt: callers
    /// have `take`n it out of `self` already.
    fn command_matches(prompt: &Prompt, surface: &Surface) -> usize {
        if prompt.mode != Mode::Command || prompt.buffer.is_empty() {
            return 0;
        }
        overlay::filter_commands(&surface.palette_commands, &prompt.buffer).len()
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
        self.lua_history_index = None;
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

    /// Insert IME-committed text into the active prompt, if any.
    /// Returns whether a prompt consumed the text. The Lua prompt owns an
    /// editor instead of the buffer, so it never consumes here — the caller
    /// routes to the editor directly.
    pub fn insert_text(&mut self, text: &str) -> bool {
        let Some(prompt) = self.prompt.as_mut() else {
            return false;
        };
        if prompt.mode == Mode::Lua {
            return false;
        }
        if text.is_empty() {
            return true;
        }
        prompt.insert(text);
        true
    }

    /// Submit a Lua-prompt line from the editor: same history, `py`
    /// environment, and message semantics as the legacy Enter path.
    pub fn submit_lua(&mut self, surface: &mut Surface, input: &str) {
        let input = input.trim().to_string();
        self.prompt = None;
        if input.is_empty() {
            return;
        }
        self.push_history(Mode::Lua, input.clone());
        self.evaluate_lua(surface, &input);
    }

    /// Walk Lua history for the editor prompt: `back` for older, otherwise
    /// newer. Returns the entry to display (`Some("")` clears the field),
    /// or `None` when there is nowhere to go.
    pub fn lua_history_step(&mut self, back: bool) -> Option<String> {
        if back {
            if self.lua_history.is_empty() {
                return None;
            }
            match self.lua_history_index {
                None => self.lua_history_index = Some(self.lua_history.len() - 1),
                Some(index) if index > 0 => self.lua_history_index = Some(index - 1),
                _ => return None,
            }
            Some(self.lua_history[self.lua_history_index.expect("just set")].clone())
        } else {
            match self.lua_history_index {
                Some(index) if index + 1 < self.lua_history.len() => {
                    self.lua_history_index = Some(index + 1);
                    Some(self.lua_history[index + 1].clone())
                }
                Some(_) => {
                    self.lua_history_index = None;
                    Some(String::new())
                }
                None => None,
            }
        }
    }

    /// Active prompt parts for the GPUI status bar: mode, prefix, full buffer,
    /// and the cursor as a character index into the buffer.
    pub fn prompt_parts(&self) -> Option<(Mode, &'static str, &str, usize)> {
        let prompt = self.prompt.as_ref()?;
        let cursor_col = prompt.buffer[..prompt.cursor.min(prompt.buffer.len())]
            .chars()
            .count();
        Some((prompt.mode, prompt.mode.prompt_text(), &prompt.buffer, cursor_col))
    }

    /// Handle a key-down while the prompt is active.
    ///
    /// Same keys as the previous implementation: Escape cancels, Enter submits
    /// (command → palette dispatch, rename → rename active, lua → evaluate),
    /// Backspace/Delete/Home/End/arrows edit, Up/Down walk history, and
    /// anything producing text inserts it.
    pub fn handle_key(
        &mut self,
        surface: &mut Surface,
        surface_entity: &Entity<Surface>,
        event: &KeyDownEvent,
        window: &mut gpui::Window,
        cx: &mut App,
    ) {
        let Some(mut prompt) = self.prompt.take() else {
            return;
        };
        let key = event.keystroke.key.as_str();
        let mut keep = true;
        match key {
            "escape" => keep = false,
            "enter" => {
                let input = prompt.buffer.trim().to_string();
                keep = false;
                if !input.is_empty() {
                    match prompt.mode {
                        Mode::Command => {
                            self.push_history(Mode::Command, input.clone());
                            self.execute_command(surface, surface_entity, window, cx, &input);
                        }
                        Mode::Rename => {
                            surface.rename_active(&input);
                        }
                        Mode::Lua => {
                            self.push_history(Mode::Lua, input.clone());
                            self.evaluate_lua(surface, &input);
                        }
                    }
                }
            }
            "backspace" => prompt.delete_before(),
            "delete" => prompt.delete_at(),
            "left" => prompt.cursor_left(),
            "right" => prompt.cursor_right(),
            "tab" => {
                // Command mode only (rename keeps Tab inert, Lua mode never
                // reaches this handler): complete the highlighted match,
                // like the old palette Tab.
                if prompt.mode == Mode::Command {
                    surface.refresh_palette_commands();
                    let selected = prompt.completion_index;
                    if let Some(name) = overlay::complete_command_name(
                        &surface.palette_commands,
                        &prompt.buffer,
                        selected,
                    ) {
                        prompt.accept_completion(name);
                    }
                }
            }
            "up" => {
                if prompt.mode == Mode::Command
                    && StatusBar::command_matches(&prompt, surface) > 0
                {
                    let matches = StatusBar::command_matches(&prompt, surface);
                    prompt.move_completion(matches, false);
                } else {
                    let history = self.history_for(prompt.mode);
                    prompt.history_prev(history);
                }
            }
            "down" => {
                if prompt.mode == Mode::Command
                    && StatusBar::command_matches(&prompt, surface) > 0
                {
                    let matches = StatusBar::command_matches(&prompt, surface);
                    prompt.move_completion(matches, true);
                } else {
                    let history = self.history_for(prompt.mode);
                    prompt.history_next(history);
                }
            }
            "home" => prompt.cursor = 0,
            "end" => prompt.cursor = prompt.buffer.len(),
            _ => {
                // Only printable text lands in the buffer; control/platform
                // chords (bound actions) never do.
                if !event.keystroke.modifiers.control && !event.keystroke.modifiers.platform {
                    if let Some(text) = event.keystroke.key_char.as_deref()
                        && !text.is_empty()
                    {
                        prompt.insert(text);
                    }
                }
            }
        }
        if keep {
            self.prompt = Some(prompt);
        }
    }

    fn history_for(&self, mode: Mode) -> &[String] {
        match mode {
            Mode::Command => &self.command_history,
            Mode::Rename => &[],
            Mode::Lua => &self.lua_history,
        }
    }

    fn push_history(&mut self, mode: Mode, entry: String) {
        let history = match mode {
            Mode::Command => &mut self.command_history,
            Mode::Rename => return,
            Mode::Lua => &mut self.lua_history,
        };
        if history.last() != Some(&entry) {
            history.push(entry);
        }
    }

    fn execute_command(
        &mut self,
        surface: &mut Surface,
        surface_entity: &Entity<Surface>,
        window: &mut gpui::Window,
        cx: &mut App,
        input: &str,
    ) {
        // The palette command list lives on the surface; refresh it so Lua
        // `register` calls made since the last prompt are visible.
        surface.refresh_palette_commands();
        if !overlay::execute_command(surface, surface_entity.clone(), window, cx, input) {
            self.message = Some((
                Instant::now(),
                format!(
                    "unknown command: {}",
                    input.split(' ').next().unwrap_or(input)
                ),
            ));
        }
    }

    fn evaluate_lua(&mut self, surface: &mut Surface, expr: &str) {
        let lua = surface.lua.clone();
        let out = config::with_env(surface, |_| {
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

pub struct Prompt {
    mode: Mode,
    buffer: String,
    cursor: usize,
    history_index: Option<usize>,
    completion_index: usize,
}

impl Prompt {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            buffer: String::new(),
            cursor: 0,
            history_index: None,
            completion_index: 0,
        }
    }

    fn insert(&mut self, text: &str) {
        self.buffer.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.completion_index = 0;
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
        self.completion_index = 0;
    }

    fn delete_at(&mut self) {
        if self.cursor >= self.buffer.len() {
            return;
        }
        self.buffer.remove(self.cursor);
        self.completion_index = 0;
    }

    /// Accept a completion, replacing the whole line (the old palette Tab
    /// dropped partial args the same way).
    fn accept_completion(&mut self, name: String) {
        self.buffer = name;
        self.cursor = self.buffer.len();
        self.completion_index = 0;
    }

    /// Move the completion highlight, wrapping around `matches` rows.
    fn move_completion(&mut self, matches: usize, down: bool) {
        if matches == 0 {
            return;
        }
        let selected = self.completion_index.min(matches - 1);
        self.completion_index = if down {
            (selected + 1) % matches
        } else {
            (selected + matches - 1) % matches
        };
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

    fn history_prev(&mut self, history: &[String]) {
        if history.is_empty() {
            return;
        }
        match self.history_index {
            None => self.history_index = Some(history.len() - 1),
            Some(index) if index > 0 => self.history_index = Some(index - 1),
            _ => return,
        }
        let index = self.history_index.expect("just set");
        self.buffer = history[index].clone();
        self.cursor = self.buffer.len();
        self.completion_index = 0;
    }

    fn history_next(&mut self, history: &[String]) {
        match self.history_index {
            Some(index) if index + 1 < history.len() => {
                self.history_index = Some(index + 1);
                self.buffer = history[index + 1].clone();
            }
            Some(_) => {
                self.history_index = None;
                self.buffer.clear();
            }
            None => return,
        }
        self.cursor = self.buffer.len();
        self.completion_index = 0;
    }

    /// Visible window of the buffer for a `cols`-wide field, with the cursor
    /// as a column offset into it. Kept from the wgpu status renderer so the
    /// truncation behavior stays tested even though the GPUI bar renders the
    /// full buffer.
    #[cfg(test)]
    fn display(&self, cols: u16) -> (String, String, usize) {
        let left_limit = usize::from(cols.max(1));

        let prompt_text = self.mode.prompt_text();
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

        (prompt_text.to_string(), visible, prompt_cols + visible_cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_insert_and_delete_are_char_aware() {
        let mut prompt = Prompt::new(Mode::Command);
        prompt.insert("héllo");
        assert_eq!(prompt.cursor, "héllo".len());
        prompt.cursor_left();
        prompt.delete_before();
        assert_eq!(prompt.buffer, "hélo");
    }

    #[test]
    fn prompt_history_walks_and_resets() {
        let history = vec!["first".to_string(), "second".to_string()];
        let mut prompt = Prompt::new(Mode::Command);
        prompt.history_prev(&history);
        assert_eq!(prompt.buffer, "second");
        prompt.history_prev(&history);
        assert_eq!(prompt.buffer, "first");
        prompt.history_prev(&history);
        assert_eq!(prompt.buffer, "first");
        prompt.history_next(&history);
        assert_eq!(prompt.buffer, "second");
        prompt.history_next(&history);
        assert!(prompt.buffer.is_empty());
    }

    #[test]
    fn display_truncates_to_width() {
        let mut prompt = Prompt::new(Mode::Command);
        prompt.insert("switch 12");
        let (prefix, visible, cursor) = prompt.display(8);
        assert_eq!(prefix, " : ");
        // 8 cols minus the 3-col prefix leaves a 5-wide window scrolled to
        // the cursor at the end of the 9-char buffer.
        assert_eq!(visible, "h 12");
        assert_eq!(cursor, 7);
    }

    #[test]
    fn insert_text_targets_active_prompt() {
        let mut status = StatusBar::new();
        assert!(!status.insert_text("hi"));
        status.open(Mode::Command);
        assert!(status.insert_text("sw"));
        assert!(status.insert_text("itch"));
        let (_, _, buffer, cursor) = status.prompt_parts().unwrap();
        assert_eq!(buffer, "switch");
        assert_eq!(cursor, 6);
        // Empty text into an active prompt is a consumed no-op.
        assert!(status.insert_text(""));
        // Lua mode owns an editor instead of the buffer: never consumes here.
        status.open(Mode::Lua);
        assert!(!status.insert_text("hi"));
    }

    #[test]
    fn mode_prefixes_match_history() {
        assert_eq!(Mode::Command.prompt_text(), " : ");
        assert_eq!(Mode::Rename.prompt_text(), " R: ");
        assert_eq!(Mode::Lua.prompt_text(), " > ");
    }
}
