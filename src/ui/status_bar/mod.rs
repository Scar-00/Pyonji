//! Status bar: prompt, tabs and transient messages.
//!
//! Data-model split (same as t3chat `Chat` / `ChatHistory`):
//! - `StatusBarState` holds all interactive state and logic. It is owned as
//!   `Entity<StatusBarState>` (by `Surface`) and never renders itself.
//! - `StatusBar` is a snapshot view (`#[derive(IntoElement)]` +
//!   `RenderOnce`) built per-frame from the entity. Leaf rows inside it
//!   (`CompletionMenu`, `ListRow`) are snapshots too.
//!
//! Prompt text editing lives in gpui-component inputs (`CommandPrompt`,
//! `RenamePrompt`, `LuaSingleLineInput` — each `Entity<T: Render>`); this
//! module owns *what* the prompt is doing (mode/history/completion/message,
//! tabs snapshot, palette commands, focus staging). `Surface` pushes
//! snapshots (`set_tabs`, `set_ssh_sessions`, …) during render and performs
//! cross-component side effects (command dispatch, rename, Lua evaluation
//! with the `py` environment, terminal refocus) from the `Submit`/`Cancel`
//! events the inputs emit.

use gpui::{App, Context, Entity, FocusHandle, Pixels, div, prelude::*, px};
use gpui_base::h_flex;

use crate::{
    lua_input::{COMPLETION_MENU_LIMIT, LuaSingleLineInput, lua_match_labels},
    overlay::{Cmd, LuaAction, complete_command_name, filter_commands},
    prompt_input::{
        CommandCancel, CommandDown, CommandPrompt, CommandPromptEvent, CommandTabAccept,
        CommandUp, RenameCancel, RenamePrompt, RenamePromptEvent,
    },
    pty::SshConnection,
    status::{Mode, StatusBar as PromptState},
    ui::completion_menu::CompletionMenu,
};

/// Max rows in the hand-rolled command completion menu.
const COMMAND_MENU_LIMIT: usize = 8;

/// Status bar height in logical pixels: `status_height` rows of `line_height`.
pub fn bar_height_px(status_height: f32, line_height: f32) -> f32 {
    (status_height.max(0.5) * line_height).max(1.0)
}

pub fn height_pixels(status_height: f32, line_height: f32) -> Pixels {
    px(bar_height_px(status_height, line_height))
}

pub struct StatusBarState {
    focus_handle: FocusHandle,
    prompt: PromptState,
    status_height: f32,
    status_bar_hidden: bool,
    lua_editor: Entity<LuaSingleLineInput>,
    command_prompt: Entity<CommandPrompt>,
    rename_prompt: Entity<RenamePrompt>,
    lua_prompt_needs_focus: bool,
    command_prompt_needs_focus: bool,
    rename_prompt_needs_focus: bool,
    rename_initial: Option<String>,
    palette_commands: Vec<Cmd>,
    tabs: Vec<(String, bool)>,
    line_height: f32,
    registered_callbacks: Vec<LuaAction>,
    ssh_sessions: Vec<SshConnection>,
    lua: mlua::Lua,
}

impl StatusBarState {
    pub fn new(
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
        lua: mlua::Lua,
        registered_callbacks: &[LuaAction],
        line_height: f32,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let lua_editor = cx.new(|cx| LuaSingleLineInput::new_with_lua(lua.clone(), window, cx));
        let command_prompt = cx.new(|cx| CommandPrompt::new(window, cx));
        let rename_prompt = cx.new(|cx| RenamePrompt::new(window, cx));

        let mut this = Self {
            focus_handle,
            prompt: PromptState::new(),
            status_height: 1.0,
            status_bar_hidden: false,
            lua_editor,
            command_prompt,
            rename_prompt,
            lua_prompt_needs_focus: false,
            command_prompt_needs_focus: false,
            rename_prompt_needs_focus: false,
            rename_initial: None,
            palette_commands: Vec::new(),
            tabs: Vec::new(),
            line_height,
            registered_callbacks: registered_callbacks.to_vec(),
            ssh_sessions: Vec::new(),
            lua,
        };

        // Reset the command completion highlight whenever the text changes.
        // Submit/Cancel are owned by `Surface` (it executes + refocuses).
        let command = this.command_prompt.clone();
        cx.subscribe_in(&command, window, |this: &mut Self, _, event, _, cx| {
            if matches!(event, CommandPromptEvent::Change) {
                this.prompt.reset_completion();
                cx.notify();
            }
        })
        .detach();
        let rename = this.rename_prompt.clone();
        cx.subscribe_in(&rename, window, |_: &mut Self, _, event, _, cx| {
            if matches!(event, RenamePromptEvent::Change) {
                cx.notify();
            }
        })
        .detach();

        cx.bind_keys([
            gpui::KeyBinding::new("tab", crate::LuaTabAccept, Some("Input")),
            gpui::KeyBinding::new("escape", crate::LuaCancelPrompt, Some("status-lua")),
            gpui::KeyBinding::new("ctrl-p", crate::LuaHistoryPrev, Some("status-lua")),
            gpui::KeyBinding::new("ctrl-n", crate::LuaHistoryNext, Some("status-lua")),
        ]);

        // Command prompt keys. `Up`/`Down` are bound under `Input` (later
        // registration runs first, like `LuaTabAccept`) because single-line
        // `MoveUp`/`MoveDown` consume them without propagating. `Tab`/`Escape`
        // propagate from single-line inputs, so wrapper-context bindings
        // suffice for them.
        cx.bind_keys([
            gpui::KeyBinding::new("up", CommandUp, Some("Input")),
            gpui::KeyBinding::new("down", CommandDown, Some("Input")),
            gpui::KeyBinding::new("escape", CommandCancel, Some("status-command")),
            gpui::KeyBinding::new("tab", CommandTabAccept, Some("status-command")),
            gpui::KeyBinding::new("escape", RenameCancel, Some("status-rename")),
        ]);

        cx.bind_keys([
            gpui::KeyBinding::new("tab", gpui::NoAction {}, Some("terminal")),
            gpui::KeyBinding::new("shift-tab", gpui::NoAction {}, Some("terminal")),
        ]);

        this.refresh_palette_commands();
        this
    }

    pub fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn set_status_height(&mut self, height: f32) {
        self.status_height = height.max(0.5);
    }

    pub fn set_status_bar_hidden(&mut self, hidden: bool) {
        self.status_bar_hidden = hidden;
    }

    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    pub fn palette_commands(&self) -> &[Cmd] {
        &self.palette_commands
    }

    pub fn lua_editor(&self) -> Entity<LuaSingleLineInput> {
        self.lua_editor.clone()
    }

    pub fn command_prompt(&self) -> Entity<CommandPrompt> {
        self.command_prompt.clone()
    }

    pub fn rename_prompt(&self) -> Entity<RenamePrompt> {
        self.rename_prompt.clone()
    }

    pub fn lua(&self) -> mlua::Lua {
        self.lua.clone()
    }

    pub fn is_active(&self) -> bool {
        self.prompt.is_active()
    }

    pub fn prompt_mode(&self) -> Option<Mode> {
        self.prompt.prompt_mode()
    }

    pub fn completion_selected(&self) -> usize {
        self.prompt.completion_selected()
    }

    /// Current command line (for tests and menu computation).
    pub fn command_value(&self, cx: &App) -> String {
        self.command_prompt.read(cx).value(cx).to_string()
    }

    /// Current rename line.
    pub fn rename_value(&self, cx: &App) -> String {
        self.rename_prompt.read(cx).value(cx).to_string()
    }

    pub fn message(&self) -> Option<String> {
        self.prompt.message().map(str::to_string)
    }

    pub fn tabs(&self) -> Vec<(String, bool)> {
        self.tabs.clone()
    }

    pub fn set_tabs(&mut self, tabs: Vec<(String, bool)>) {
        self.tabs = tabs;
    }

    pub fn set_line_height(&mut self, line_height: f32) {
        self.line_height = line_height;
    }

    pub fn set_ssh_sessions(&mut self, sessions: Vec<SshConnection>) {
        self.ssh_sessions = sessions;
        self.refresh_palette_commands();
    }

    pub fn refresh_palette_commands(&mut self) {
        self.palette_commands = crate::overlay::commands_for_surface(
            &self.prompt,
            &self.registered_callbacks,
            &self.lua,
            &self.ssh_sessions,
        );
    }

    pub fn set_registered_callbacks(&mut self, callbacks: Vec<LuaAction>) {
        self.registered_callbacks = callbacks;
        self.refresh_palette_commands();
    }

    pub fn push_registered_callback(&mut self, action: LuaAction) {
        self.registered_callbacks.push(action);
        self.refresh_palette_commands();
    }

    pub fn clear_registered_callbacks(&mut self) {
        self.registered_callbacks.clear();
        self.refresh_palette_commands();
    }

    pub fn show_status_message(&mut self, text: String, cx: &mut Context<Self>) {
        self.prompt.show_message(text);
        cx.notify();
        Self::arm_message_timer(cx);
    }

    pub fn show_unknown_command(&mut self, input: &str, cx: &mut Context<Self>) {
        self.prompt.show_unknown_command(input);
        cx.notify();
        Self::arm_message_timer(cx);
    }

    pub fn push_command_history(&mut self, entry: String) {
        self.prompt.push_history(Mode::Command, entry);
    }

    pub fn push_lua_history(&mut self, entry: String) {
        self.prompt.push_history(Mode::Lua, entry);
    }

    fn arm_message_timer(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            smol::Timer::after(crate::status::MESSAGE_TIMEOUT).await;
            let _ = this.update(cx, |this: &mut StatusBarState, cx| {
                if this.prompt.clear_message_if_expired() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Atomically take pending focus requests for the snapshot render.
    /// Returns `(lua, command, rename, rename_initial)`.
    fn take_focus_requests(&mut self) -> (bool, bool, bool, Option<String>) {
        let lua = std::mem::take(&mut self.lua_prompt_needs_focus);
        let command = std::mem::take(&mut self.command_prompt_needs_focus);
        let rename = std::mem::take(&mut self.rename_prompt_needs_focus);
        let initial = self.rename_initial.take();
        (lua, command, rename, initial)
    }

    /// Complete a Lua editor submit: close the prompt, record history and
    /// clear the field. Returns the code for the owner (`Surface`) to
    /// evaluate with the `py` environment. Focusing the terminal is the
    /// owner's job (it owns that focus handle).
    pub fn complete_lua_submit(
        &mut self,
        code: &str,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let submitted = self.prompt.take_lua_submit(code)?;
        self.lua_editor.update(cx, |editor, cx| {
            editor.set_value("", window, cx);
        });
        cx.notify();
        Some(submitted)
    }

    /// Complete a command submit: close, record history, clear the field.
    /// Returns the line for the owner to execute, or `None` when empty.
    pub fn complete_command_submit(
        &mut self,
        code: &str,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let submitted = self.prompt.take_command_submit(code)?;
        self.command_prompt.update(cx, |prompt, cx| {
            prompt.set_value("", window, cx);
        });
        cx.notify();
        Some(submitted)
    }

    /// Complete a rename submit: close and clear. Returns the name when
    /// non-empty (no history for renames).
    pub fn complete_rename_submit(
        &mut self,
        name: &str,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let submitted = self.prompt.take_rename_submit(name)?;
        self.rename_prompt.update(cx, |prompt, cx| {
            prompt.set_value("", window, cx);
        });
        cx.notify();
        Some(submitted)
    }

    /// Cancel any prompt and clear its field. Returns whether one was active.
    /// Refocusing the terminal stays with the owner.
    pub fn cancel_any_prompt(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.prompt.is_active() {
            return false;
        }
        self.prompt.cancel_prompt();
        self.command_prompt.update(cx, |prompt, cx| {
            prompt.set_value("", window, cx);
        });
        self.rename_prompt.update(cx, |prompt, cx| {
            prompt.set_value("", window, cx);
        });
        self.lua_editor.update(cx, |editor, cx| {
            editor.set_value("", window, cx);
        });
        cx.notify();
        true
    }

    pub fn accept_lua_completion(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let editor = self.lua_editor.clone();
        if self.prompt.prompt_mode() != Some(Mode::Lua) || !editor.read(cx).is_focused(window, cx)
        {
            cx.propagate();
            return;
        }
        window.prevent_default();
        let value = editor.read(cx).value(cx).to_string();
        let offset = editor
            .read(cx)
            .state()
            .read(cx)
            .cursor()
            .min(value.len());
        if !value.is_char_boundary(offset) {
            return;
        }
        let before = &value[..offset];
        let labels =
            lua_match_labels(editor.read(cx).lsp().lua(), before, COMPLETION_MENU_LIMIT);
        let Some(label) = labels.into_iter().next() else {
            return;
        };
        let (_, prefix) = crate::lua_input::split_completion_target(before);
        let prefix_start = offset - prefix.len();
        let mut completed = String::with_capacity(value.len() + label.len());
        completed.push_str(&value[..prefix_start]);
        completed.push_str(&label);
        completed.push_str(&value[offset..]);
        editor.update(cx, |editor, cx| {
            editor.set_value(&completed, window, cx);
        });
        cx.notify();
    }

    /// Cancel the Lua prompt without focusing anyone; the owner refocuses
    /// the terminal. Returns whether a prompt was active.
    pub fn cancel_lua_prompt(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.prompt.prompt_mode() != Some(Mode::Lua) {
            return false;
        }
        window.prevent_default();
        cx.stop_propagation();
        self.prompt.cancel_prompt();
        self.lua_editor.update(cx, |editor, cx| {
            editor.set_value("", window, cx);
        });
        cx.notify();
        true
    }

    pub fn lua_history_step(
        &mut self,
        back: bool,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        window.prevent_default();
        cx.stop_propagation();
        if let Some(text) = self.prompt.lua_history_step(back) {
            let editor = self.lua_editor.clone();
            editor.update(cx, |editor, cx| {
                editor.set_value(&text, window, cx);
            });
        }
        cx.notify();
    }

    fn command_matches(&self, cx: &App) -> Vec<Cmd> {
        let value = self.command_prompt.read(cx).value(cx).to_string();
        if value.is_empty() {
            return Vec::new();
        }
        filter_commands(&self.palette_commands, &value)
    }

    /// Accept the highlighted command match, replacing the whole line (the
    /// old palette Tab dropped partial args the same way).
    pub fn accept_command_completion(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.prompt.prompt_mode() != Some(Mode::Command) {
            cx.propagate();
            return;
        }
        let prompt = self.command_prompt.clone();
        if !prompt.read(cx).is_focused(window, cx) {
            cx.propagate();
            return;
        }
        window.prevent_default();
        cx.stop_propagation();
        let value = prompt.read(cx).value(cx).to_string();
        let selected = self.prompt.completion_selected();
        let Some(name) = complete_command_name(&self.palette_commands, &value, selected) else {
            return;
        };
        prompt.update(cx, |prompt, cx| {
            prompt.set_value(&name, window, cx);
        });
        self.prompt.reset_completion();
        cx.notify();
    }

    /// Move the command completion highlight, wrapping around matches.
    pub fn move_command_completion(
        &mut self,
        down: bool,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        window.prevent_default();
        cx.stop_propagation();
        let matches = self.command_matches(cx).len();
        if matches == 0 {
            return;
        }
        let selected = self.prompt.completion_selected().min(matches - 1);
        let next = if down {
            (selected + 1) % matches
        } else {
            (selected + matches - 1) % matches
        };
        self.prompt.set_completion_selected(next);
        cx.notify();
    }

    /// Navigate command history (`back` for older). Resets the completion
    /// highlight like the old buffer path did.
    pub fn command_history_step(
        &mut self,
        back: bool,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        window.prevent_default();
        cx.stop_propagation();
        if let Some(text) = self.prompt.command_history_step(back) {
            let prompt = self.command_prompt.clone();
            prompt.update(cx, |prompt, cx| {
                prompt.set_value(&text, window, cx);
            });
            self.prompt.reset_completion();
        }
        cx.notify();
    }

    /// Shared `Up`/`Down` for the command prompt: with matches, navigate the
    /// menu; otherwise walk history. Propagates when the command prompt is
    /// not focused so other inputs (Lua, overlays) keep their own keys.
    pub fn command_vertical(
        &mut self,
        down: bool,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if self.prompt.prompt_mode() != Some(Mode::Command) {
            cx.propagate();
            return;
        }
        let prompt = self.command_prompt.clone();
        if !prompt.read(cx).is_focused(window, cx) {
            cx.propagate();
            return;
        }
        if self.command_matches(cx).is_empty() {
            self.command_history_step(down == false, window, cx);
        } else {
            self.move_command_completion(down, window, cx);
        }
    }

    pub fn open_status_prompt(&mut self, cx: &mut Context<Self>) {
        self.prompt.open(Mode::Command);
        self.command_prompt_needs_focus = true;
        self.refresh_palette_commands();
        cx.notify();
    }

    pub fn open_rename_prompt(&mut self, cx: &mut Context<Self>, name: String) {
        self.prompt.open_rename();
        self.rename_initial = Some(name);
        self.rename_prompt_needs_focus = true;
        cx.notify();
    }

    pub fn open_lua_prompt(&mut self, cx: &mut Context<Self>) {
        self.prompt.open(Mode::Lua);
        self.lua_prompt_needs_focus = true;
        cx.notify();
    }

    pub fn cancel_prompt(&mut self, cx: &mut Context<Self>) {
        self.prompt.cancel_prompt();
        cx.notify();
    }

    /// Handle a key while a prompt is active. The inputs own their keys now
    /// (they are focused); this only ensures focus lands in the active input
    /// when the terminal still holds it (e.g. the frame before `defer`
    /// runs) and consumes the key so it never reaches the pty.
    pub fn handle_key_down(
        &mut self,
        _event: &gpui::KeyDownEvent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        if !self.prompt.is_active() {
            return;
        }
        match self.prompt.prompt_mode() {
            Some(Mode::Command) => {
                let prompt = self.command_prompt.clone();
                prompt.update(cx, |prompt, cx| prompt.focus(window, cx));
            }
            Some(Mode::Rename) => {
                let prompt = self.rename_prompt.clone();
                prompt.update(cx, |prompt, cx| prompt.focus(window, cx));
            }
            Some(Mode::Lua) => {
                let editor = self.lua_editor.clone();
                editor.update(cx, |editor, cx| editor.focus(window, cx));
            }
            None => {}
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    pub fn handle_key_repeat(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_key_down(event, window, cx);
    }

    pub fn commit_text(
        &mut self,
        text: &str,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        match self.prompt.prompt_mode() {
            Some(Mode::Command) => {
                let prompt = self.command_prompt.clone();
                prompt.update(cx, |prompt, cx| {
                    prompt.insert_text(text, window, cx);
                });
                cx.notify();
            }
            Some(Mode::Rename) => {
                let prompt = self.rename_prompt.clone();
                prompt.update(cx, |prompt, cx| {
                    prompt.insert_text(text, window, cx);
                });
                cx.notify();
            }
            Some(Mode::Lua) => {
                let editor = self.lua_editor.clone();
                let current = editor.read(cx).value(cx).to_string();
                editor.update(cx, |editor, cx| {
                    editor.set_value(&format!("{current}{text}"), window, cx);
                });
                cx.notify();
            }
            None => {}
        }
    }
}

#[derive(IntoElement)]
pub struct StatusBar {
    state: Entity<StatusBarState>,
}

impl StatusBar {
    pub fn new(state: &Entity<StatusBarState>) -> Self {
        Self {
            state: state.clone(),
        }
    }
}

impl RenderOnce for StatusBar {
    fn render(self, window: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        // Focus prompts that opened without a window at hand (Lua API stages
        // the request; the next frame owns the window).
        let (lua_focus, command_focus, rename_focus, rename_initial) =
            self.state.update(cx, |s, _| s.take_focus_requests());
        if lua_focus {
            let editor = self.state.read(cx).lua_editor();
            window.defer(cx, move |window, cx| {
                editor.update(cx, |editor, cx| editor.focus(window, cx));
            });
        }
        if command_focus {
            let prompt = self.state.read(cx).command_prompt();
            window.defer(cx, move |window, cx| {
                prompt.update(cx, |prompt, cx| {
                    prompt.set_value("", window, cx);
                    prompt.focus(window, cx);
                });
            });
        }
        if rename_focus {
            let prompt = self.state.read(cx).command_prompt();
            let rename = self.state.read(cx).rename_prompt();
            let _ = prompt;
            window.defer(cx, move |window, cx| {
                rename.update(cx, |prompt, cx| {
                    prompt.set_value_select_all(&rename_initial.unwrap_or_default(), window, cx);
                    prompt.focus(window, cx);
                });
            });
        }

        self.render_inner(window, cx)
    }
}

impl StatusBar {
    fn render_inner(&self, _window: &mut gpui::Window, cx: &mut App) -> gpui::AnyElement {
        let st = self.state.read(cx);
        if st.status_bar_hidden {
            return div().into_any_element();
        }
        let bar_h = height_pixels(st.status_height, st.line_height);

        if st.prompt.prompt_mode() == Some(Mode::Lua) {
            let state = self.state.clone();
            let editor = st.lua_editor.clone();
            let matches = {
                let value = editor.read(cx).value(cx).to_string();
                let cursor = {
                    let state_entity = editor.read(cx).state().clone();
                    state_entity.read(cx).cursor()
                };
                let offset = cursor.min(value.len());
                let before = value[..offset.min(value.len())].to_string();
                lua_match_labels(editor.read(cx).lsp().lua(), &before, COMPLETION_MENU_LIMIT)
            };
            let editor_slot = div()
                .id("status-lua")
                .key_context("status-lua")
                .on_action({
                    let state = state.clone();
                    move |_: &crate::LuaTabAccept, window: &mut gpui::Window, cx: &mut App| {
                        state.update(cx, |bar, cx| {
                            bar.accept_lua_completion(window, cx);
                        });
                    }
                })
                .on_action({
                    let state = state.clone();
                    move |_: &crate::LuaCancelPrompt, window: &mut gpui::Window, cx: &mut App| {
                        state.update(cx, |bar, cx| {
                            let _ = bar.cancel_lua_prompt(window, cx);
                        });
                    }
                })
                .on_action({
                    let state = state.clone();
                    move |_: &crate::LuaHistoryPrev, window: &mut gpui::Window, cx: &mut App| {
                        state.update(cx, |bar, cx| {
                            bar.lua_history_step(true, window, cx);
                        });
                    }
                })
                .on_action({
                    let state = state.clone();
                    move |_: &crate::LuaHistoryNext, window: &mut gpui::Window, cx: &mut App| {
                        state.update(cx, |bar, cx| {
                            bar.lua_history_step(false, window, cx);
                        });
                    }
                })
                .child(editor)
                .into_any_element();
            return render_lua(bar_h, matches, editor_slot);
        }

        if st.prompt.prompt_mode() == Some(Mode::Command) {
            let state = self.state.clone();
            let prompt = st.command_prompt.clone();
            let value = prompt.read(cx).value(cx).to_string();
            let completions: Vec<String> = if value.is_empty() {
                Vec::new()
            } else {
                filter_commands(&st.palette_commands, &value)
                    .into_iter()
                    .take(COMMAND_MENU_LIMIT)
                    .map(|cmd| cmd.hint())
                    .collect()
            };
            let completion_selected = st.prompt.completion_selected();
            let editor_slot = div()
                .id("status-command")
                .key_context("status-command")
                .on_action({
                    let state = state.clone();
                    move |_: &CommandUp, window: &mut gpui::Window, cx: &mut App| {
                        state.update(cx, |bar, cx| {
                            bar.command_vertical(false, window, cx);
                        });
                    }
                })
                .on_action({
                    let state = state.clone();
                    move |_: &CommandDown, window: &mut gpui::Window, cx: &mut App| {
                        state.update(cx, |bar, cx| {
                            bar.command_vertical(true, window, cx);
                        });
                    }
                })
                .on_action({
                    let state = state.clone();
                    move |_: &CommandTabAccept, window: &mut gpui::Window, cx: &mut App| {
                        state.update(cx, |bar, cx| {
                            bar.accept_command_completion(window, cx);
                        });
                    }
                })
                .on_action({
                    let state = state.clone();
                    move |_: &CommandCancel, window: &mut gpui::Window, cx: &mut App| {
                        window.prevent_default();
                        cx.stop_propagation();
                        let prompt = state.read(cx).command_prompt();
                        prompt.update(cx, |prompt, cx| {
                            prompt.emit_cancel(cx);
                        });
                    }
                })
                .child(prompt)
                .into_any_element();
            return render_command(bar_h, completions, completion_selected, editor_slot);
        }

        if st.prompt.prompt_mode() == Some(Mode::Rename) {
            let state = self.state.clone();
            let prompt = st.rename_prompt.clone();
            let editor_slot = div()
                .id("status-rename")
                .key_context("status-rename")
                .on_action({
                    let state = state.clone();
                    move |_: &RenameCancel, window: &mut gpui::Window, cx: &mut App| {
                        window.prevent_default();
                        cx.stop_propagation();
                        let prompt = state.read(cx).rename_prompt();
                        prompt.update(cx, |prompt, cx| {
                            prompt.emit_cancel(cx);
                        });
                    }
                })
                .child(prompt)
                .into_any_element();
            return render_rename(bar_h, editor_slot);
        }

        let message = st.prompt.message().map(str::to_string);
        let tabs = st.tabs.clone();
        render_tabs(bar_h, tabs, message)
    }
}

fn render_tabs(
    height: Pixels,
    tabs: Vec<(String, bool)>,
    message: Option<String>,
) -> gpui::AnyElement {
    h_flex()
        .w_full()
        .text_sm()
        .px(crate::theme::space::_1)
        .h(height)
        .rounded(crate::theme::radius::SM)
        .items_center()
        .bg(crate::theme::role::bar_bg())
        .children(tabs.into_iter().map(|(label, is_active)| {
            div()
                .px(crate::theme::space::_2)
                .py(crate::theme::space::_0_5)
                .mr(crate::theme::space::_1)
                .rounded(crate::theme::radius::SM)
                .when(is_active, |this| {
                    this.bg(crate::theme::role::accent())
                        .text_color(crate::theme::role::accent_fg())
                })
                .when(!is_active, |this| {
                    this.bg(crate::theme::role::surface())
                        .text_color(crate::theme::role::text())
                })
                .child(label)
        }))
        .when_some(message, |this, message| {
            this.child(
                div()
                    .ml_auto()
                    .px(crate::theme::space::_2)
                    .py(crate::theme::space::_0_5)
                    .rounded(crate::theme::radius::SM)
                    .bg(crate::theme::role::message())
                    .text_color(crate::theme::role::message_fg())
                    .child(message),
            )
        })
        .into_any_element()
}

fn render_command(
    height: Pixels,
    completions: Vec<String>,
    completion_selected: usize,
    editor_slot: gpui::AnyElement,
) -> gpui::AnyElement {
    use gpui_base::v_flex;
    v_flex()
        .w_full()
        .text_sm()
        .child(CompletionMenu::new(completions, completion_selected))
        .child(
            div()
                .w_full()
                .h(height)
                .flex()
                .flex_col()
                .justify_center()
                .child(editor_slot),
        )
        .into_any_element()
}

fn render_rename(height: Pixels, editor_slot: gpui::AnyElement) -> gpui::AnyElement {
    use gpui_base::v_flex;
    v_flex().w_full().text_sm().child(
        div()
            .w_full()
            .h(height)
            .flex()
            .flex_col()
            .justify_center()
            .child(editor_slot),
    ).into_any_element()
}

fn render_lua(
    height: Pixels,
    matches: Vec<String>,
    editor_slot: gpui::AnyElement,
) -> gpui::AnyElement {
    use gpui_base::v_flex;
    v_flex()
        .w_full()
        .text_sm()
        .child(CompletionMenu::new(matches, 0))
        .child(
            div()
                .w_full()
                .h(height)
                .flex()
                .flex_col()
                .justify_center()
                .child(editor_slot),
        )
        .into_any_element()
}

#[allow(dead_code)]
fn _theme(_: &App) {}
