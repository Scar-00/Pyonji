//! Status prompt state: mode, history, completion selection and messages.
//!
//! Text editing itself lives in gpui-component inputs (`CommandPrompt`,
//! `RenamePrompt`, `LuaSingleLineInput`); this owns *what* the prompt is
//! doing without knowing about `Surface`, windows or Lua. Callers (the
//! `StatusBar` entity and `Surface`) perform side effects (command dispatch,
//! rename, Lua evaluation).

use std::time::{Duration, Instant};

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
    prompt: Option<Mode>,
    message: Option<(Instant, String)>,
    command_history: Vec<String>,
    lua_history: Vec<String>,
    command_history_index: Option<usize>,
    lua_history_index: Option<usize>,
    command_completion_selected: usize,
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
            command_history_index: None,
            lua_history_index: None,
            command_completion_selected: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.prompt.is_some()
    }

    /// Mode of the active prompt, if any.
    pub fn prompt_mode(&self) -> Option<Mode> {
        self.prompt
    }

    /// Close the prompt without submitting (also resets transient indexes).
    pub fn cancel_prompt(&mut self) {
        self.prompt = None;
        self.command_history_index = None;
        self.lua_history_index = None;
        self.command_completion_selected = 0;
    }

    /// Highlighted completion row for the command menu.
    pub fn completion_selected(&self) -> usize {
        self.command_completion_selected
    }

    pub fn set_completion_selected(&mut self, selected: usize) {
        self.command_completion_selected = selected;
    }

    pub fn reset_completion(&mut self) {
        self.command_completion_selected = 0;
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_ref().map(|(_, text)| text.as_str())
    }

    pub fn show_message(&mut self, text: String) {
        self.message = Some((Instant::now(), text));
    }

    pub fn show_unknown_command(&mut self, input: &str) {
        self.message = Some((
            Instant::now(),
            format!(
                "unknown command: {}",
                input.split(' ').next().unwrap_or(input)
            ),
        ));
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
        self.command_history_index = None;
        self.lua_history_index = None;
        self.command_completion_selected = 0;
        self.prompt = Some(mode);
    }

    pub fn open_rename(&mut self) {
        self.open(Mode::Rename);
    }

    /// Close a command submit: record history when non-empty.
    /// Returns the code for the owner to execute, or `None` when empty.
    pub fn take_command_submit(&mut self, input: &str) -> Option<String> {
        if self.prompt != Some(Mode::Command) {
            return None;
        }
        let input = input.trim().to_string();
        self.prompt = None;
        self.command_history_index = None;
        self.command_completion_selected = 0;
        if input.is_empty() {
            return None;
        }
        self.push_history(Mode::Command, input.clone());
        Some(input)
    }

    /// Close a rename submit (no history). Returns the name when non-empty.
    pub fn take_rename_submit(&mut self, input: &str) -> Option<String> {
        if self.prompt != Some(Mode::Rename) {
            return None;
        }
        let input = input.trim().to_string();
        self.prompt = None;
        if input.is_empty() {
            return None;
        }
        Some(input)
    }

    /// Close a Lua editor prompt without evaluating. Returns whether a
    /// Lua prompt was active.
    pub fn take_lua_submit(&mut self, input: &str) -> Option<String> {
        if self.prompt != Some(Mode::Lua) {
            return None;
        }
        let input = input.trim().to_string();
        self.prompt = None;
        self.lua_history_index = None;
        if input.is_empty() {
            return None;
        }
        self.push_history(Mode::Lua, input.clone());
        Some(input)
    }

    /// Walk command history: `back` for older, otherwise newer.
    pub fn command_history_step(&mut self, back: bool) -> Option<String> {
        Self::history_step(
            &self.command_history,
            &mut self.command_history_index,
            back,
        )
    }

    /// Walk Lua history for the editor prompt.
    pub fn lua_history_step(&mut self, back: bool) -> Option<String> {
        Self::history_step(&self.lua_history, &mut self.lua_history_index, back)
    }

    fn history_step(
        history: &[String],
        index: &mut Option<usize>,
        back: bool,
    ) -> Option<String> {
        if back {
            if history.is_empty() {
                return None;
            }
            match *index {
                None => *index = Some(history.len() - 1),
                Some(i) if i > 0 => *index = Some(i - 1),
                _ => return None,
            }
            Some(history[index.expect("just set")].clone())
        } else {
            match *index {
                Some(i) if i + 1 < history.len() => {
                    *index = Some(i + 1);
                    Some(history[i + 1].clone())
                }
                Some(_) => {
                    *index = None;
                    Some(String::new())
                }
                None => None,
            }
        }
    }

    pub fn push_history(&mut self, mode: Mode, entry: String) {
        let history = match mode {
            Mode::Command => &mut self.command_history,
            Mode::Rename => return,
            Mode::Lua => &mut self.lua_history,
        };
        if history.last() != Some(&entry) {
            history.push(entry);
        }
        // A new entry invalidates any in-progress walk.
        match mode {
            Mode::Command => self.command_history_index = None,
            Mode::Lua => self.lua_history_index = None,
            Mode::Rename => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_submit_records_history_and_closes() {
        let mut status = StatusBar::new();
        status.open(Mode::Command);
        assert!(status.is_active());
        assert_eq!(
            status.take_command_submit("  switch 1  ").as_deref(),
            Some("switch 1")
        );
        assert!(!status.is_active());
        // Empty submit just closes.
        status.open(Mode::Command);
        assert_eq!(status.take_command_submit("   "), None);
        assert!(!status.is_active());
        // Wrong mode never submits.
        status.open(Mode::Rename);
        assert_eq!(status.take_command_submit("switch"), None);
    }

    #[test]
    fn rename_submit_has_no_history() {
        let mut status = StatusBar::new();
        status.open_rename();
        assert_eq!(status.prompt_mode(), Some(Mode::Rename));
        assert_eq!(status.take_rename_submit("  new-name ").as_deref(), Some("new-name"));
        assert!(!status.is_active());
        status.open_rename();
        assert_eq!(status.take_rename_submit("   "), None);
    }

    #[test]
    fn command_history_walks_and_resets() {
        let mut status = StatusBar::new();
        status.push_history(Mode::Command, "first".to_string());
        status.push_history(Mode::Command, "second".to_string());
        // Consecutive duplicates are ignored.
        status.push_history(Mode::Command, "second".to_string());
        assert_eq!(status.command_history_step(true).as_deref(), Some("second"));
        assert_eq!(status.command_history_step(true).as_deref(), Some("first"));
        assert_eq!(status.command_history_step(true), None);
        assert_eq!(status.command_history_step(false).as_deref(), Some("second"));
        assert_eq!(status.command_history_step(false).as_deref(), Some(""));
        assert_eq!(status.command_history_step(false), None);
    }

    #[test]
    fn lua_history_steps_like_command() {
        let mut status = StatusBar::new();
        status.push_history(Mode::Lua, "a".to_string());
        status.push_history(Mode::Lua, "b".to_string());
        assert_eq!(status.lua_history_step(true).as_deref(), Some("b"));
        assert_eq!(status.lua_history_step(false).as_deref(), Some(""));
    }

    #[test]
    fn cancel_resets_transient_state() {
        let mut status = StatusBar::new();
        status.open(Mode::Command);
        status.set_completion_selected(3);
        status.push_history(Mode::Command, "x".to_string());
        status.command_history_step(true);
        status.cancel_prompt();
        assert!(!status.is_active());
        assert_eq!(status.completion_selected(), 0);
    }

    #[test]
    fn mode_prefixes_match_history() {
        assert_eq!(Mode::Command.prompt_text(), " : ");
        assert_eq!(Mode::Rename.prompt_text(), " R: ");
        assert_eq!(Mode::Lua.prompt_text(), " > ");
    }
}
