#![cfg_attr(all(windows, feature = "install"), windows_subsystem = "windows")]
#![allow(
    clippy::similar_names,
    clippy::ptr_as_ptr,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::cast_sign_loss,
    clippy::struct_excessive_bools,
    clippy::type_complexity
)]

mod config;
mod input;
mod lua_input;
#[cfg(feature = "install")]
mod logging;
mod overlay;
mod prompt_input;
mod pty;
mod renderer;
mod status;
mod terminal;
mod theme;
mod ui;
mod workspace;

use std::{collections::HashMap, fmt::Display, ops::Range, panic::Location, path::{Path, PathBuf}, sync::Arc};
#[cfg(not(feature = "install"))]
use tracing_subscriber::prelude::*;

use anyhow::Result;
use clap::Parser;
use gpui::{
    Action, App, Bounds, ClipboardItem, Context, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, TextInputConfiguration, UTF16Selection, Window, actions, canvas, container_query, div, point, prelude::*, px, size
};
use gpui_component::{Root, Theme, ThemeMode, v_flex};
use mlua::{
    Lua, LuaOptions, StdLib,
    prelude::{LuaFunction, LuaMultiValue, LuaTable},
};

use crate::{
    config::KeyBinding,
    input::{InputManager, bare_key, digit_index},
    overlay::{LuaAction, Overlay, OverlayHost, Screen},
    pty::{Event as PtyEvent, SshConnection},
    status::Mode,
    terminal::{SessionId, SplitDirection, TerminalSession},
    ui::{status_bar::StatusBar, TerminalState},
    workspace::Workspace,
};

// Actions for the Lua status prompt, scoped to the `status-lua` key context
// on the prompt wrapper. The editor owns Up/Down/Enter/Escape under its
// deeper `Input` context (Up/Down move the cursor, Enter submits, Escape is
// unconsumed without a menu), so history lives on Ctrl+P/Ctrl+N
// (readline-style, no conflicts) and Tab accepts the first completion
// (single-line editors propagate Tab instead of indenting).
actions!(status_lua, [
    LuaCancelPrompt,
    LuaHistoryPrev,
    LuaHistoryNext
]);

// Tab gate for the Lua prompt, bound under the editor's own `Input`
// context. Same depth as the component's `IndentInline`, but registered
// later, so it runs first: with our Lua editor focused it accepts the
// first match, otherwise it propagates and normal Tab behavior proceeds
// (other inputs, palettes, focus navigation).
actions!([LuaTabAccept]);

// Deferred UI intents. Key paths dispatch these directly (they hold a
// window); Lua without a window pushes them into `pending_actions`
// (constructing an action needs no context) for `render` to dispatch.
// Either way one shared `.on_action` handler performs the work, so there
// is a single vocabulary instead of bespoke staging enums.
actions!(pyonji, [
    OpenCommandPrompt,
    OpenLuaPrompt,
    ToggleStatusBar,
    ToggleFullscreen
]);

/// Open an overlay dialog for `screen`. Queued by Lua, dispatched
/// directly by key paths — the handler calls `Overlay::open`.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = pyonji, no_json)]
pub struct OpenOverlay {
    pub screen: Screen,
}

/// Open the rename prompt prefilled with `name`.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = pyonji, no_json)]
pub struct OpenRenamePrompt {
    pub name: String,
}

/// Show a transient status message with the standard expiry.
#[derive(Clone, PartialEq, Debug, Action)]
#[action(namespace = pyonji, no_json)]
pub struct ShowStatusMessage {
    pub text: String,
}

#[derive(clap::Parser)]
struct Cli {
    path: Option<PathBuf>,
}

/// One-shot UI intents are GPUI actions now (see the `pyonji` actions
/// above): key paths dispatch them directly, Lua without a window queues
/// them in `pending_actions` for `render` to dispatch. Window-chrome
/// sizing lives in `TerminalState` as plain owned data, synced into the
/// bar each frame like the font metrics.
pub struct Surface {
    pub focus_handle: FocusHandle,
    pub lua: Lua,
    pub event_tx: async_channel::Sender<PtyEvent>,
    initial_path: Option<PathBuf>,

    /// Session/tab data model (what is open).
    pub workspace: Workspace,
    /// Terminal viewport, GPU resources and pointer gestures (how it looks).
    pub terminal: TerminalState,
    /// Keymap and transient key modes.
    pub input: InputManager,

    pub status_bar: Entity<StatusBar>,
    pub overlay_host: Entity<OverlayHost>,

    /// Lua-queued UI intents. Constructing an action needs no window or
    /// app context, so Lua without either can still request UI work;
    /// `render` drains the queue through `window.dispatch_action`, which
    /// runs the same `.on_action` handlers the key paths dispatch to
    /// directly. The mailbox itself is the only transient state left —
    /// the GPUI analogue of `cx.defer` for a producer outside GPUI.
    pending_actions: Vec<Box<dyn Action>>,

    /// Lua-registered palette actions (source of truth for sync Lua API;
    /// `StatusBar` holds a copy for display, synced every frame in
    /// `render`). Lives here (not only in the bar) so `py:register` works
    /// without a window/cx.
    pub registered_callbacks: Vec<LuaAction>,

    pub fullscreen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinAction {
    Action,
    Palette,
    Sessions,
    Tab(usize),
    NextTab,
    PrevTab,
    NextPane,
    SplitVertical,
    SplitHorizontal,
    ToggleDecorations,
    ToggleStatusBar,
    StatusPrompt,
    LuaPrompt,
    MoveToTab,
    DetachSession,
    RenameSession,
    Detached,
}

#[derive(Debug, Clone)]
pub enum KeyAction {
    Builtin(BuiltinAction),
    Custom(LuaFunction),
}

fn main() -> Result<()> {
    cfg_select! {
        feature = "install" => {
            logging::init();
        }
        _ => {
            tracing_subscriber::registry()
                .with(tracing_subscriber::fmt::layer())
                .with(tracing_subscriber::filter::LevelFilter::WARN)
                .init();
        }
    };
    let cli = Cli::parse();

    let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL_SAFE, LuaOptions::new()) };
    config::install_inspect(&lua)?;
    for (name, source) in config::LUA_MODULES {
        let function = lua.load(*source).into_function()?;
        let package: LuaTable = lua.globals().get("package")?;
        let preload: LuaTable = package.get("preload")?;
        preload.set(*name, function.clone())?;
        function.call::<()>(())?;
    }

    let (event_tx, event_rx) = async_channel::unbounded::<PtyEvent>();
    {
        let tx = event_tx.clone();
        let print = lua.create_function(move |lua, args: LuaMultiValue| {
            let tostring: LuaFunction = lua.globals().get("tostring")?;
            let mut parts = Vec::new();
            for value in args {
                let text: String = tostring.call(value)?;
                parts.push(text);
            }
            _ = tx.try_send(PtyEvent::LuaPrint(parts.join("\t")));
            Ok(())
        })?;
        lua.globals().set("print", print)?;
        lua.globals().set("trace", lua.create_function(|lua, args: LuaMultiValue| {
            let tostring: LuaFunction = lua.globals().get("tostring")?;
            let mut parts = Vec::new();
            for value in args {
                let text: String = tostring.call(value)?;
                parts.push(text);
            }
            tracing::error!("{}", parts.join("\t"));
            Ok(())
        })?)?;
    }

    config::watch(event_tx.clone());

    let icon: Option<Arc<image::RgbaImage>> = image::load_from_memory(Surface::ICON)
        .map(|image| Arc::new(image.to_rgba8()))
        .inspect_err(|error| tracing::error!(%error, "failed to decode window icon"))
        .ok();

    let initial_path = cli.path.clone();
    let lua_for_app = lua.clone();
    let tx_for_app = event_tx.clone();

    gpui_platform::application()
        .with_assets(gpui_component_assets::Assets)
        .run(move |cx| {
            gpui_component::init(cx);
            let lua = lua_for_app.clone();
            let event_tx = tx_for_app.clone();
            let initial_path = initial_path.clone();
            let event_rx = event_rx.clone();
            let icon = icon.clone();
            // Center the initial window on the primary display. Tiling
            // compositors ignore placement (and size), floating ones honor it.
            let (initial_width, initial_height) = Surface::INITIAL_SIZE;
            let initial_size = size(px(initial_width), px(initial_height));
            let initial_origin = cx
                .primary_display()
                .map(|display| {
                    let bounds = display.bounds();
                    let x = (f32::from(bounds.size.width) - initial_width).max(0.0) / 2.0
                        + f32::from(bounds.origin.x);
                    let y = (f32::from(bounds.size.height) - initial_height).max(0.0) / 2.0
                        + f32::from(bounds.origin.y);
                    point(px(x), px(y))
                })
                .unwrap_or_default();
            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::new(
                        initial_origin,
                        initial_size,
                    ))),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some(Surface::TITLE.into()),
                        appears_transparent: false,
                        traffic_light_position: None,
                    }),
                    // Wayland app-id, so compositors can classify/group the
                    // window (previously the class came from the winit build).
                    app_id: Some("pyonji".to_string()),
                    icon,
                    ..Default::default()
                },
                |window, cx| {
                    Theme::change(ThemeMode::Dark, Some(window), cx);
                    let surface = cx.new(|cx| {
                        Surface::new(window, cx, initial_path, lua, event_tx, event_rx)
                    });
                    surface.update(cx, |surface, _| {
                        config::load(surface);
                        surface.ensure_initial_session();
                    });

                    // Config loads after the window exists, so a
                    // `fullscreen = true` config cannot shape the initial
                    // bounds. Apply it directly — the window is in scope.
                    if surface.read(cx).fullscreen {
                        window.toggle_fullscreen();
                    }

                    let focus = surface.read(cx).focus_handle(cx);
                    window.defer(cx, move |window, cx| {
                        if window.focused(cx).is_none() {
                            focus.focus(window, cx);
                        }
                    });
                    cx.new(|cx| Root::new(surface, window, cx))
                },
            )
            .expect("failed to open window");
        });
    Ok(())
}

impl Surface {
    const TITLE: &str = cfg_select! {
        feature = "install" => "Pyonji",
        _ => {
            const_format::formatcp!("Pyonji {}", git_version::git_version!())
        },
    };
    /// Runtime window icon, decoded from the same `.ico` the Windows build
    /// embeds via `build.rs`. GPUI only consumes it on X11; Wayland uses the
    /// `.desktop` entry, Windows the exe resources, macOS the app bundle.
    const ICON: &[u8] = include_bytes!("../resources/icon.ico");
    /// Initial windowed size, matching the old winit window.
    const INITIAL_SIZE: (f32, f32) = (1280.0, 720.0);

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        initial_path: Option<PathBuf>,
        lua: Lua,
        event_tx: async_channel::Sender<PtyEvent>,
        event_rx: async_channel::Receiver<PtyEvent>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let workspace = Workspace::new(event_tx.clone());
        let terminal = TerminalState::new();
        let input = InputManager::new();
        let line_height = terminal.line_height;
        let status_bar = cx.new(|cx| {
            StatusBar::new(window, cx, lua.clone(), &[], line_height)
        });
        let overlay_host = cx.new(|_cx| OverlayHost::new());
        let surface = Self {
            focus_handle,
            lua,
            event_tx,
            initial_path,
            workspace,
            terminal,
            input,
            status_bar: status_bar.clone(),
            overlay_host: overlay_host.clone(),
            pending_actions: Vec::new(),
            registered_callbacks: Vec::new(),
            fullscreen: false,
        };

        // Lua editor submits (Enter) are owned by the StatusBar entity, but
        // evaluation needs the `py` environment which only `Surface` can
        // provide. Subscribe here so Surface performs evaluation with its
        // Lua state and updates history/messages via the bar.
        let editor = surface.status_bar.read(cx).lua_editor();
        cx.subscribe_in(&editor, window, |surface: &mut Surface, _: &Entity<crate::lua_input::LuaSingleLineInput>, event: &crate::lua_input::LuaInputEvent, window: &mut Window, cx: &mut Context<Surface>| {
            if let crate::lua_input::LuaInputEvent::SubmitRequested(code) = event {
                let code = code.clone();
                surface.submit_lua_from_editor(&code, window, cx);
            }
        })
        .detach();

        // Command and rename prompts are `InputState` editors owned by the
        // bar; execution/rename plus terminal refocus live here (the bar
        // doesn't own terminal focus or the palette dispatch).
        let command = surface.status_bar.read(cx).command_prompt();
        cx.subscribe_in(
            &command,
            window,
            |surface: &mut Surface,
             _: &Entity<crate::prompt_input::CommandPrompt>,
             event: &crate::prompt_input::CommandPromptEvent,
             window: &mut Window,
             cx: &mut Context<Surface>| {
                match event {
                    crate::prompt_input::CommandPromptEvent::Submit(code) => {
                        let code = code.clone().to_string();
                        surface.submit_command_from_prompt(&code, window, cx);
                    }
                    crate::prompt_input::CommandPromptEvent::Cancel => {
                        surface.cancel_prompt_and_refocus(window, cx);
                    }
                    crate::prompt_input::CommandPromptEvent::Change => {}
                }
            },
        )
        .detach();
        let rename = surface.status_bar.read(cx).rename_prompt();
        cx.subscribe_in(
            &rename,
            window,
            |surface: &mut Surface,
             _: &Entity<crate::prompt_input::RenamePrompt>,
             event: &crate::prompt_input::RenamePromptEvent,
             window: &mut Window,
             cx: &mut Context<Surface>| {
                match event {
                    crate::prompt_input::RenamePromptEvent::Submit(name) => {
                        let name = name.clone().to_string();
                        surface.submit_rename_from_prompt(&name, window, cx);
                    }
                    crate::prompt_input::RenamePromptEvent::Cancel => {
                        surface.cancel_prompt_and_refocus(window, cx);
                    }
                    crate::prompt_input::RenamePromptEvent::Change => {}
                }
            },
        )
        .detach();

        // Background pty/config events → surface updates. Mirrors the old
        // event-loop `user_event` handler.
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = event_rx.recv().await {
                let done = this
                    .update_in(cx, |this: &mut Surface, window, cx| {
                        this.handle_pty_event(event, window, cx)
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        })
        .detach();

        surface
    }

    /// Boot the first session after `config::load`, so `default_cwd` (and the
    /// CLI path, which wins) applies — same order as the old event-loop
    /// startup, where config loaded before the initial session was created.
    pub fn ensure_initial_session(&mut self) {
        let initial = self.initial_path.clone();
        self.workspace.ensure_initial_session(initial.as_deref());
    }

    /// Queue an overlay dialog to open on the next frame. Constructing the
    /// `OpenOverlay` action needs no window, so Lua shares this path;
    /// `render` dispatches the queue. Key paths with a window dispatch
    /// directly instead.
    pub fn request_overlay(&mut self, screen: Screen) {
        self.queue_action(OpenOverlay { screen });
    }

    /// Queue a UI intent from Lua (no window/cx in scope). `render` drains
    /// the queue through `window.dispatch_action`, running the same
    /// `.on_action` handlers the key paths use directly.
    pub fn queue_action(&mut self, action: impl Action) {
        self.pending_actions.push(Box::new(action));
    }

    /// Returns `true` when the app should stop listening (quit requested).
    fn handle_pty_event(&mut self, event: PtyEvent, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        match event {
            PtyEvent::Closed(id) => {
                if self.close_session(id) && self.workspace.session_manager.is_empty() {
                    cx.quit();
                    return true;
                }
                cx.notify();
            }
            PtyEvent::Data(id, data) => {
                self.workspace.session_manager.update_session(id, &data);
                cx.notify();
            }
            PtyEvent::ProgramChanged((id, title)) => {
                if let Some(session) = self.workspace.session_manager.session_mut(id) {
                    session.set_title(title);
                    cx.notify();
                }
            }
            PtyEvent::ConfigChanged => {
                config::load(self);
                cx.notify();
            }
            PtyEvent::LuaPrint(text) => {
                self.show_status_message(text.replace(['\n', '\r'], " "), cx);
            }
            PtyEvent::ReleasesReady(releases) => {
                self.overlay_host
                    .update(cx, |host, _| host.set_releases(releases));
            }
            PtyEvent::Exit => {
                cx.quit();
                return true;
            }
        }
        false
    }

    pub fn default_keymap() -> HashMap<KeyBinding, KeyAction> {
        crate::input::default_keymap()
    }

    /// Show a transient status message with the standard 5s expiry.
    pub fn show_status_message(&mut self, text: String, cx: &mut Context<Self>) {
        self.status_bar
            .update(cx, |bar, cx| bar.show_status_message(text, cx));
    }

    /// Run a Lua-prompt line submitted from the editor: history and field
    /// clearing live in the `StatusBar` component, evaluation (with the
    /// `py` environment) lives here, then focus returns to the terminal.
    pub fn submit_lua_from_editor(
        &mut self,
        code: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bar = self.status_bar.clone();
        let submitted = bar.update(cx, |bar, cx| {
            bar.complete_lua_submit(code, window, cx)
        });
        let Some(code) = submitted else {
            let focus = self.focus_handle.clone();
            window.focus(&focus, cx);
            return;
        };
        let message = self.evaluate_lua(&code);
        self.status_bar.update(cx, |bar, cx| {
            bar.show_status_message(message, cx);
        });
        let focus = self.focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Evaluate one Lua line with the `py` environment, formatting the
    /// result like the REPL. Errors become their message.
    pub fn evaluate_lua(&mut self, expr: &str) -> String {
        use mlua::prelude::{LuaFunction, LuaValue};
        let lua = self.lua.clone();
        let out = crate::config::with_env(self, |_| {
            let value = lua.load(expr).eval::<LuaValue>()?;
            let res = Self::format_lua_value(&lua, value, 0)?;
            Ok(res)
        });
        let message = match out {
            Err(error) => format!("{error}"),
            Ok(message) => message,
        };
        message.replace(['\n', '\r'], " ")
    }

    fn format_lua_value(
        lua: &Lua,
        value: mlua::Value,
        depth: usize,
    ) -> mlua::Result<String> {
        use mlua::prelude::{LuaFunction, LuaValue};
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

    /// Cancel the Lua prompt: clear via the bar, refocus the terminal here
    /// (the bar doesn't own terminal focus).
    pub fn cancel_lua_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bar = self.status_bar.clone();
        let was_active = bar.update(cx, |bar, cx| bar.cancel_lua_prompt(window, cx));
        if was_active {
            let focus = self.focus_handle.clone();
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    /// Run a command-prompt line: history/clearing in the bar, dispatch here.
    /// Empty lines just close and refocus.
    pub fn submit_command_from_prompt(
        &mut self,
        code: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bar = self.status_bar.clone();
        let submitted = bar.update(cx, |bar, cx| {
            bar.complete_command_submit(code, window, cx)
        });
        let Some(line) = submitted else {
            let focus = self.focus_handle.clone();
            window.focus(&focus, cx);
            cx.notify();
            return;
        };
        let entity = cx.entity();
        let commands = self.status_bar.read(cx).palette_commands().to_vec();
        if !crate::overlay::execute_command(&commands, entity, window, cx, &line) {
            self.status_bar.update(cx, |bar, cx| {
                bar.show_unknown_command(&line, cx);
            });
        }
        let focus = self.focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Rename the active session from the rename prompt, then refocus.
    pub fn submit_rename_from_prompt(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bar = self.status_bar.clone();
        let submitted =
            bar.update(cx, |bar, cx| bar.complete_rename_submit(name, window, cx));
        if let Some(name) = submitted {
            self.workspace.rename_active(&name);
        }
        let focus = self.focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Cancel any prompt (command/rename/Lua): clear via the bar, refocus.
    pub fn cancel_prompt_and_refocus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bar = self.status_bar.clone();
        let was_active = bar.update(cx, |bar, cx| bar.cancel_any_prompt(window, cx));
        if was_active {
            let focus = self.focus_handle.clone();
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    /// Prompt openers shared by the `.on_action` handlers below. Unhide the
    /// chrome here (visibility now lives in `TerminalState`) and open the
    /// prompt in the bar.
    pub fn open_status_prompt(&mut self, cx: &mut Context<Self>) {
        self.terminal.status_hidden = false;
        self.status_bar
            .update(cx, |bar, cx| bar.open_status_prompt(cx));
    }

    pub fn open_lua_prompt(&mut self, cx: &mut Context<Self>) {
        self.terminal.status_hidden = false;
        self.status_bar
            .update(cx, |bar, cx| bar.open_lua_prompt(cx));
    }

    pub fn open_rename_prompt(&mut self, cx: &mut Context<Self>) {
        let name = self
            .workspace
            .active_session()
            .and_then(|id| self.workspace.session_manager.session(id))
            .map(TerminalSession::title)
            .unwrap_or_default()
            .to_owned();
        self.open_rename_prompt_with(cx, name);
    }

    pub fn open_rename_prompt_with(&mut self, cx: &mut Context<Self>, name: String) {
        self.terminal.status_hidden = false;
        self.status_bar
            .update(cx, |bar, cx| bar.open_rename_prompt(cx, name));
    }

    /// Toggle chrome visibility. Synchronous plain-data flip plus resize —
    /// no staging needed.
    pub fn toggle_status_bar(&mut self, cx: &mut Context<Self>) {
        self.terminal.status_hidden = !self.terminal.status_hidden;
        self.resize_tab();
        cx.notify();
    }
    fn dispatch_builtin(
        &mut self,
        action: BuiltinAction,
        _entity: Entity<Surface>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // UI intents go through the shared GPUI actions (dispatched here —
        // the key path holds a window), so key and Lua flows run the same
        // `.on_action` handlers. Plain-data input modes and workspace ops
        // stay synchronous.
        match action {
            BuiltinAction::Action => {
                self.input.enter_action_mode();
            }
            BuiltinAction::Palette => {
                window.dispatch_action(Box::new(OpenOverlay { screen: Screen::CmdPalette }), cx);
            }
            BuiltinAction::Sessions => {
                window.dispatch_action(Box::new(OpenOverlay { screen: Screen::Sessions }), cx);
            }
            BuiltinAction::Tab(index) => {
                let _ = self.switch_tab(index);
            }
            BuiltinAction::NextTab => {
                let index = self.workspace.next_tab_index();
                let _ = self.switch_tab(index);
            }
            BuiltinAction::PrevTab => {
                let index = self.workspace.previous_tab_index();
                let _ = self.switch_tab(index);
            }
            BuiltinAction::NextPane => {
                let _ = self.workspace.focus_next_pane();
            }
            BuiltinAction::SplitVertical => {
                let _ = self.split_current_tab(SplitDirection::Vertical);
            }
            BuiltinAction::SplitHorizontal => {
                let _ = self.split_current_tab(SplitDirection::Horizontal);
            }
            BuiltinAction::ToggleDecorations => {
                // GPUI owns window chrome; see the matching Lua stub.
                window.dispatch_action(
                    Box::new(ShowStatusMessage {
                        text: "window decorations toggle is not supported on GPUI".to_string(),
                    }),
                    cx,
                );
            }
            BuiltinAction::ToggleStatusBar => {
                window.dispatch_action(Box::new(ToggleStatusBar), cx);
            }
            BuiltinAction::StatusPrompt => {
                window.dispatch_action(Box::new(OpenCommandPrompt), cx);
            }
            BuiltinAction::LuaPrompt => {
                window.dispatch_action(Box::new(OpenLuaPrompt), cx);
            }
            BuiltinAction::MoveToTab => {
                self.input.begin_move_to_tab();
            }
            BuiltinAction::DetachSession => {
                let _ = self.detach_active_session();
            }
            BuiltinAction::RenameSession => {
                let name = self
                    .workspace
                    .active_session()
                    .and_then(|id| self.workspace.session_manager.session(id))
                    .map(TerminalSession::title)
                    .unwrap_or_default()
                    .to_owned();
                window.dispatch_action(Box::new(OpenRenamePrompt { name }), cx);
            }
            BuiltinAction::Detached => {
                window.dispatch_action(Box::new(OpenOverlay { screen: Screen::Detached }), cx);
            }
        }
        cx.notify();
    }
}

impl Surface {
    pub fn terminal_rows(&self) -> u16 {
        self.terminal.terminal_rows()
    }

    pub fn cols(&self) -> u16 {
        self.terminal.cols
    }

    pub fn rows(&self) -> u16 {
        self.terminal.rows
    }

    // --- Workspace facades: same signatures as before, delegating to the
    // --- owned `Workspace` component with viewport dims. Keeps Lua,
    // --- palette and key paths unchanged while data lives in `workspace`.

    pub fn active_session(&self) -> Option<SessionId> {
        self.workspace.active_session()
    }

    pub fn next_tab_index(&self) -> usize {
        self.workspace.next_tab_index()
    }

    pub fn previous_tab_index(&self) -> usize {
        self.workspace.previous_tab_index()
    }

    fn resize_tab(&mut self) {
        let cols = self.terminal.cols;
        let rows = self.terminal_rows();
        self.workspace.resize_tab(cols, rows);
    }

    fn set_active_session(&mut self, session_id: SessionId) {
        self.workspace.set_active_session(session_id);
    }

    fn focus_next_pane(&mut self) -> Option<SessionId> {
        self.workspace.focus_next_pane()
    }

    fn resize_active_pane(&mut self, direction: SplitDirection, delta_first: i16) {
        let cols = self.terminal.cols;
        let rows = self.terminal_rows();
        self.workspace
            .resize_active_pane(direction, delta_first, cols, rows);
    }

    fn split_current_tab(&mut self, direction: SplitDirection) -> Option<SessionId> {
        let cols = self.terminal.cols;
        let rows = self.terminal_rows();
        self.workspace.split_current_tab(direction, cols, rows)
    }

    pub fn switch_tab(&mut self, tab: usize) -> bool {
        let cols = self.terminal.cols;
        let rows = self.terminal_rows();
        self.workspace.switch_tab(tab, cols, rows)
    }

    fn move_session_to_tab(&mut self, session: SessionId, target: usize) -> bool {
        let cols = self.terminal.cols;
        let rows = self.terminal_rows();
        self.workspace
            .move_session_to_tab(session, target, cols, rows)
    }

    pub fn detach_active_session(&mut self) -> bool {
        let cols = self.terminal.cols;
        let rows = self.terminal_rows();
        self.workspace.detach_active_session(cols, rows)
    }

    pub fn close_session(&mut self, session: SessionId) -> bool {
        let cols = self.terminal.cols;
        let rows = self.terminal_rows();
        self.workspace.close_session(session, cols, rows)
    }

    pub fn reattach_session(&mut self, session: SessionId, target: usize) -> bool {
        let cols = self.terminal.cols;
        let rows = self.terminal_rows();
        self.workspace.reattach_session(session, target, cols, rows)
    }

    pub fn live_detached_sessions(&self) -> Vec<SessionId> {
        self.workspace.live_detached_sessions()
    }

    fn rename_session(&mut self, session: SessionId, name: &str) -> bool {
        self.workspace.rename_session(session, name)
    }

    fn take_wheel_steps(&mut self, delta_lines: f32) -> i32 {
        self.workspace.take_wheel_steps(delta_lines)
    }

    pub fn open_session_in_dir(&mut self, path: &Path) {
        let cols = self.terminal.cols;
        let rows = self.terminal_rows();
        self.workspace.open_session_in_dir(path, cols, rows);
    }

    pub fn create_remote_session(&mut self, session: &SshConnection) {
        let cols = self.terminal.cols;
        let rows = self.terminal_rows();
        self.workspace.create_remote_session(session, cols, rows);
    }

    fn sync_surface(&mut self, window: &mut Window, content_size: gpui::Size<Pixels>) {
        // Split borrows: terminal owns GPU/viewport, workspace owns sessions.
        let (terminal, workspace) = (&mut self.terminal, &mut self.workspace);
        terminal.sync_surface(window, content_size, workspace);
    }

    fn paint_terminal(&mut self) {
        self.terminal.paint_terminal(&self.workspace);
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        entity: Entity<Surface>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Only the focused terminal consumes keys: typing in a dialog or the
        // status prompt must not reach the pty.
        if !self.focus_handle.is_focused(window) {
            return;
        }
        if event.is_held {
            // Held-key repeats go straight to the pty / prompt repeat path
            // below without re-triggering keybindings.
            return self.handle_key_repeat(event, entity, window, cx);
        }

        // The status prompts own their keyboards once focused (gpui-component
        // `Input`/`Editor` with their own key contexts). While one is active
        // but the terminal still holds focus (the frame before `defer`
        // moves it), focus the input and consume so nothing reaches the pty.
        // Submits arrive as `Submit` events the bar emits, handled via the
        // subscriptions in `new` — not via key return values.
        let status_active = self.status_bar.read(cx).is_active();
        if status_active || self.status_bar.read(cx).is_active() {
            self.status_bar
                .update(cx, |bar, cx| bar.handle_key_down(event, window, cx));
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
            return;
        }

        // Resize mode: arrows resize the active split while the action key is
        // held (mirrors the previous `resize_mode_held` path).
        if self.input.resize_mode_held {
            let delta = match event.keystroke.key.as_str() {
                "left" => Some((SplitDirection::Vertical, -1)),
                "right" => Some((SplitDirection::Vertical, 1)),
                "up" => Some((SplitDirection::Horizontal, -1)),
                "down" => Some((SplitDirection::Horizontal, 1)),
                _ => None,
            };
            if let Some((direction, delta)) = delta {
                self.input.resize_mode_used = true;
                self.resize_active_pane(direction, delta);
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }

        let modifiers = &event.keystroke.modifiers;
        let no_mods =
            !modifiers.control && !modifiers.alt && !modifiers.shift && !modifiers.platform;
        // A bare key while a move is pending either completes the move (digit)
        // or cancels it — consumed either way, exactly like before.
        if no_mods && self.input.pending_move_to_tab {
            self.input.pending_move_to_tab = false;
            self.input.action_mode = false;
            if let Some(target) = digit_index(event)
                && let Some(session) = self.workspace.active_session()
            {
                self.move_session_to_tab(session, target);
            }
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
            return;
        }

        let matched = self.input.match_action(event);
        if let Some(matched_action) = matched {
            let was_in_action_mode = self.input.action_mode;
            let is_action_trigger = InputManager::is_action_trigger(&matched_action);
            if was_in_action_mode && !is_action_trigger {
                self.input.action_mode = false;
            }
            // Bare keys only fire inside action mode (except the trigger
            // itself), exactly like the previous dispatch.
            if is_action_trigger || !(!was_in_action_mode && bare_key(event)) {
                match matched_action {
                    KeyAction::Custom(func) => {
                        config::with_env(self, |this| {
                            func.call::<()>(this)?;
                            Ok(())
                        })
                        .into_log();
                    }
                    KeyAction::Builtin(action) => {
                        self.dispatch_builtin(action, entity, window, cx);
                    }
                }
                // Consumed keys must stop here: GPUI delivers text separately
                // from key events, and an unstopped trigger would be
                // re-delivered as text into whatever just opened.
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
                return;
            }
        } else if self.input.action_mode {
            // Any unbound key leaves action mode, as before.
            self.input.action_mode = false;
            cx.notify();
        }

        let Some(active_session) = self.workspace.active_session() else {
            return;
        };
        let reset_scrollback = self
            .workspace
            .session_manager
            .session_mut(active_session)
            .is_some_and(TerminalSession::reset_scrollback);
        let consumed = self
            .workspace
            .session_manager
            .session_mut(active_session)
            .is_some_and(|session| session.handle_key_down(event));
        if consumed {
            window.prevent_default();
            cx.stop_propagation();
        }
        if reset_scrollback {
            cx.notify();
        }
    }

    /// Held-key repeats: never re-trigger bindings, just feed text/arrows to
    /// the active consumer (status prompt or pty).
    fn handle_key_repeat(
        &mut self,
        event: &KeyDownEvent,
        _entity: Entity<Surface>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.status_bar.read(cx).is_active() {
            self.status_bar
                .update(cx, |bar, cx| bar.handle_key_repeat(event, window, cx));
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // Resize repeats keep resizing while held.
        if self.input.resize_mode_held {
            let delta = match event.keystroke.key.as_str() {
                "left" => Some((SplitDirection::Vertical, -1)),
                "right" => Some((SplitDirection::Vertical, 1)),
                "up" => Some((SplitDirection::Horizontal, -1)),
                "down" => Some((SplitDirection::Horizontal, 1)),
                _ => None,
            };
            if let Some((direction, delta)) = delta {
                self.input.resize_mode_used = true;
                self.resize_active_pane(direction, delta);
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        if let Some(active_session) = self.workspace.active_session() {
            let consumed = self
                .workspace
                .session_manager
                .session_mut(active_session)
                .is_some_and(|session| session.handle_key_down(event));
            if consumed {
                window.prevent_default();
                cx.stop_propagation();
            }
        }
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent, cx: &mut Context<Self>) {
        // Releasing the action key ends resize/action mode, as before.
        if self.input.handle_action_key_up(&event.keystroke.key) {
            cx.notify();
        }
    }

    /// Window-relative mouse position → logical pixels relative to the
    /// terminal texture origin. Delegates to the terminal component.
    fn mouse_to_terminal(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
        self.terminal.mouse_to_terminal(position)
    }

    /// Window-relative mouse position → 1-based terminal cell.
    fn mouse_to_cell(&self, position: Point<Pixels>) -> Option<(u16, u16)> {
        self.terminal.mouse_to_cell(position)
    }

    /// Divider under a terminal-relative logical-pixel position. Delegates
    /// to the terminal component (same 6px slop).
    fn divider_hit_test(
        &self,
        x: f32,
        y: f32,
    ) -> Option<crate::terminal::Divider> {
        self.terminal
            .divider_hit_test(x, y, &self.workspace)
    }

    fn resize_dragged_divider(
        &mut self,
        drag: &crate::ui::terminal::DividerDrag,
        x: f32,
        y: f32,
    ) {
        let (terminal, workspace) = (&mut self.terminal, &mut self.workspace);
        terminal.resize_dragged_divider(drag, x, y, workspace);
    }

    fn pane_at(&self, col: u16, row: u16) -> Option<(SessionId, u16, u16)> {
        self.workspace.pane_at(
            col,
            row,
            self.terminal.cols,
            self.terminal_rows(),
        )
    }

    fn handle_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }
        // Divider drags start on left press, like before — they take over the
        // gesture so the press never reaches the pane underneath.
        if event.button == MouseButton::Left
            && let Some((x, y)) = self.mouse_to_terminal(event.position)
            && let Some(divider) = self.divider_hit_test(x, y)
        {
            let drag = crate::ui::terminal::DividerDrag {
                path: divider.path,
                direction: divider.direction,
            };
            self.resize_dragged_divider(&drag, x, y);
            self.terminal.divider_drag = Some(drag);
            cx.notify();
            return;
        }
        let Some((col, row)) = self.mouse_to_cell(event.position) else {
            return;
        };
        let Some((session_id, col, row)) = self.pane_at(col, row) else {
            return;
        };
        self.set_active_session(session_id);
        if let Some(session) = self.workspace.session_manager.session_mut(session_id) {
            let reset_scrollback = if session.uses_local_scrollback() {
                false
            } else {
                session.reset_scrollback()
            };
            session.handle_mouse_down(event.button, &event.modifiers, col, row);
            if reset_scrollback {
                cx.notify();
            }
        }
    }

    fn handle_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        // Releasing ends an in-progress divider drag (consumed, as before).
        if self.terminal.take_drag().is_some() {
            cx.notify();
            return;
        }
        let Some((col, row)) = self.mouse_to_cell(event.position) else {
            return;
        };
        let Some((session_id, col, row)) = self.pane_at(col, row) else {
            return;
        };
        if let Some(session) = self.workspace.session_manager.session_mut(session_id) {
            session.handle_mouse_up(event.button, &event.modifiers, col, row);
        }
        let _ = cx;
    }

    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button.is_none() {
            return;
        }
        // An active divider drag owns the motion, like before.
        if let Some(drag) = self.terminal.divider_drag.clone()
            && let Some((x, y)) = self.mouse_to_terminal(event.position)
        {
            self.resize_dragged_divider(&drag, x, y);
            cx.notify();
            return;
        }
        let Some((col, row)) = self.mouse_to_cell(event.position) else {
            return;
        };
        let Some((session_id, col, row)) = self.pane_at(col, row) else {
            return;
        };
        if let Some(session) = self.workspace.session_manager.session_mut(session_id) {
            session.handle_mouse_move(&event.modifiers, col, row);
        }
        let _ = cx;
    }

    fn handle_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {        let Some((col, row)) = self.mouse_to_cell(event.position) else {
            return;
        };
        let Some((session_id, col, row)) = self.pane_at(col, row) else {
            return;
        };
        let lines = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => {
                let scale = self.terminal.terminal_scale.max(f32::EPSILON);
                f32::from(pixels.y) / (self.terminal.line_height / scale)
            }
        };
        let uses_local_scrollback = self
            .workspace
            .session_manager
            .session(session_id)
            .is_some_and(TerminalSession::uses_local_scrollback);
        let whole_lines = if uses_local_scrollback {
            self.take_wheel_steps(lines)
        } else {
            self.workspace.wheel_remainder = 0.0;
            0
        };
        if let Some(session) = self.workspace.session_manager.session_mut(session_id) {
            if uses_local_scrollback {
                if whole_lines != 0 && session.scroll_scrollback(whole_lines) {
                    cx.notify();
                }
            } else {
                let reset_scrollback = session.reset_scrollback();
                session.handle_mouse_wheel(lines, &event.modifiers, col, row);
                if reset_scrollback {
                    cx.notify();
                }
            }
        }
    }
}

/* MOVED to `ui::terminal` (`grid_position`, `hit_divider`) with tests.
/// Fractional grid position of a logical-pixel point, clamped to the grid.
fn grid_position(
    x: f32,
    y: f32,
    cell_width: f32,
    line_height: f32,
    cols: u16,
    rows: u16,
) -> (f32, f32) {
    let col = (x.max(0.0) / cell_width).clamp(0.0, f32::from(cols));
    let row = (y.max(0.0) / line_height).clamp(0.0, f32::from(rows));
    (col, row)
}

/// Divider under a terminal-relative point, with grab slop around the line.
fn hit_divider(
    dividers: &[Divider],
    x: f32,
    y: f32,
    cell_width: f32,
    line_height: f32,
) -> Option<Divider> {
    const HIT_SLOP: f32 = 6.0;
    for divider in dividers {
        match divider.direction {
            SplitDirection::Vertical => {
                let line_x = cell_width * f32::from(divider.x);
                let min_y = line_height * f32::from(divider.y);
                let max_y = line_height * f32::from(divider.y + divider.rows);
                if (x - line_x).abs() <= HIT_SLOP
                    && y >= min_y - HIT_SLOP
                    && y <= max_y + HIT_SLOP
                {
                    return Some(divider.clone());
                }
            }
            SplitDirection::Horizontal => {
                let line_y = line_height * f32::from(divider.y);
                let min_x = cell_width * f32::from(divider.x);
                let max_x = cell_width * f32::from(divider.x + divider.cols);
                if (y - line_y).abs() <= HIT_SLOP
                    && x >= min_x - HIT_SLOP
                    && x <= max_x + HIT_SLOP
                {
                    return Some(divider.clone());
                }
            }
        }
    }
    None
}
*/

/* LEGACY IME HELPER REMOVED — delegated above.
impl Surface {
    /// Current IME preedit positioned at the active cursor, for the wgpu
    /// preedit renderer. Mirrors the previous `ime_preedit` helper.
    fn ime_preedit(&self) -> Option<ImePreedit> {
        let text = self.ime_preedit.clone()?;
        let active_session = self.active_session()?;
        let session = self.session_manager.session(active_session)?;
        let (_, geometry) = self
            .tab_layouts()
            .into_iter()
            .find(|(session_id, _)| *session_id == active_session)?;
        let (row, col) = session.vt.screen().cursor_position();

        Some(ImePreedit {
            text,
            geometry,
            row,
            col,
        })
    }
}
*/

/// Platform IME support for the terminal.
///
/// The terminal has no linear text buffer, so document queries (`text_for_range`,
/// `selected_text_range`, …) report nothing — the surrounding-text–free minimum
/// every toolkit accepts. What the old winit code did maps onto two callbacks:
/// composition updates (`replace_and_mark_text_in_range`, previously `Ime::Preedit`)
/// stage the preedit string for the wgpu renderer, and commits
/// (`replace_text_in_range`, previously `Ime::Commit`) send the text to the pty.
/// While a status prompt is open, committed text goes into the prompt instead,
/// which is strictly more useful than the old unconditional pty send.
/// Candidate-window placement (`bounds_for_range`) tracks the cursor cell, like
/// the old `set_ime_cursor_area` updates.
impl EntityInputHandler for Surface {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.terminal
            .ime_preedit
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        // Repaints happen every frame; nothing to notify from `App`.
        self.terminal.ime_preedit = None;
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Plain key text duplicates the KeyDown path (which already sent it),
        // so only an IME commit — one with a composition in flight — sends
        // here. Pastes bypass this via `paste` below.
        if self.terminal.ime_preedit.is_none() {
            return;
        }
        self.terminal.ime_preedit = None;
        if text.is_empty() {
            return;
        }
        self.commit_text(text, window, cx);
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = item.text() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        self.terminal.ime_preedit = None;
        self.commit_text(&text, window, cx);
    }


    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.ime_preedit = (!new_text.is_empty()).then(|| new_text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.terminal
            .ime_bounds(element_bounds, &self.workspace)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        true
    }

    fn text_input_configuration(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> TextInputConfiguration {
        TextInputConfiguration::default()
    }
}

impl Surface {
    /// Send committed text (IME commit or paste) to whoever owns input: the
    /// Lua editor, the legacy prompt, or the active pty.
    fn commit_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.status_bar
            .update(cx, |bar, cx| bar.commit_text(text, window, cx));
        // The bar routes to the Lua editor or legacy buffer when active;
        // otherwise the pty owns the text.
        if self.status_bar.read(cx).is_active() {
            cx.notify();
            return;
        }
        if text.is_empty() {
            return;
        }
        let Some(active_session) = self.workspace.active_session() else {
            return;
        };
        let reset_scrollback = self
            .workspace
            .session_manager
            .session_mut(active_session)
            .is_some_and(TerminalSession::reset_scrollback);
        self.workspace
            .session_manager
            .send_text(active_session, text);
        if reset_scrollback {
            cx.notify();
        }
    }
}

/* LEGACY HELPERS + TAB/SESSION IMPL REMOVED — moved to `input.rs`
 * (`digit_index`, `bare_key`) and `workspace.rs` (all tab/session logic).
 * Kept as a block comment to preserve history while Surface delegates
 * to its owned components.
fn digit_index(event: &KeyDownEvent) -> Option<usize> {    match event.keystroke.key.as_str() {
        "1" => Some(0),
        "2" => Some(1),
        "3" => Some(2),
        "4" => Some(3),
        "5" => Some(4),
        "6" => Some(5),
        "7" => Some(6),
        "8" => Some(7),
        "9" => Some(8),
        _ => event
            .keystroke
            .key_char
            .as_deref()
            .and_then(|ch| match ch {
                "1" => Some(0),
                "2" => Some(1),
                "3" => Some(2),
                "4" => Some(3),
                "5" => Some(4),
                "6" => Some(5),
                "7" => Some(6),
                "8" => Some(7),
                "9" => Some(8),
                _ => None,
            }),
    }
}

/// Whether the keystroke arrived without modifiers (bare action-mode key).
fn bare_key(event: &KeyDownEvent) -> bool {
    let mods = &event.keystroke.modifiers;
    !mods.control && !mods.alt && !mods.shift && !mods.platform
}

impl Surface {
    fn next_tab_index(&self) -> usize {
        (self.current_tab + 1) % self.tabs.len()
    }

    fn previous_tab_index(&self) -> usize {
        if self.current_tab == 0 {
            self.tabs.len() - 1
        } else {
            self.current_tab - 1
        }
    }

    fn switch_to_previous_live_tab_or_stay(&mut self, closed_tab: usize) {
        if let Some(tab) = (0..self.tabs.len())
            .map(|offset| (closed_tab + self.tabs.len() - 1 - offset) % self.tabs.len())
            .find(|&tab| self.tabs[tab].is_some())
        {
            self.current_tab = tab;
            self.wheel_remainder = 0.0;
            self.resize_tab();
            return;
        }

        self.current_tab = closed_tab.min(self.tabs.len().saturating_sub(1));
    }

    fn take_wheel_steps(&mut self, delta_lines: f32) -> i32 {
        let total = self.wheel_remainder + delta_lines;
        let whole = if total > 0.0 {
            total.floor() as i32
        } else if total < 0.0 {
            total.ceil() as i32
        } else {
            0
        };
        self.wheel_remainder = total - whole as f32;
        whole
    }

    fn active_session(&self) -> Option<SessionId> {
        self.tabs[self.current_tab]
            .as_ref()
            .and_then(Tab::active_session)
    }

    fn resize_tab(&mut self) {
        self.resize_tab_at(self.current_tab);
    }

    fn resize_tab_at(&mut self, index: usize) {
        let rows = self.terminal_rows();
        let Some(tab) = self.tabs[index].as_ref() else {
            return;
        };
        for (session_id, geometry) in tab.layout(PaneGeometry {
            x: 0,
            y: 0,
            cols: self.cols,
            rows,
        }) {
            self.session_manager.resize_session(
                session_id,
                geometry.rows.max(1),
                geometry.cols.max(1),
            );
        }
    }

    fn set_active_session(&mut self, session_id: SessionId) {
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if tab.set_active_session(session_id) {
            self.wheel_remainder = 0.0;
        }
    }

    fn focus_next_pane(&mut self) -> Option<SessionId> {
        let tab = self.tabs[self.current_tab].as_mut()?;
        let next = tab.focus_next()?;
        self.wheel_remainder = 0.0;
        Some(next)
    }

    fn resize_active_pane(&mut self, direction: SplitDirection, delta_first: i16) {
        let area = PaneGeometry {
            x: 0,
            y: 0,
            cols: self.cols,
            rows: self.terminal_rows(),
        };
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if !tab.resize_active_split(area, direction, delta_first) {
            return;
        }
        self.wheel_remainder = 0.0;
        self.resize_tab();
    }

    fn split_current_tab(&mut self, direction: SplitDirection) -> Option<SessionId> {
        let active_session = self.active_session()?;
        let (_, geometry) = self
            .tab_layouts()
            .into_iter()
            .find(|(session_id, _)| *session_id == active_session)?;

        let can_split = match direction {
            SplitDirection::Horizontal => geometry.rows >= 2,
            SplitDirection::Vertical => geometry.cols >= 2,
        };
        if !can_split {
            return None;
        }

        let new_rows = match direction {
            SplitDirection::Horizontal => geometry.rows / 2,
            SplitDirection::Vertical => geometry.rows,
        }
        .max(1);
        let new_cols = match direction {
            SplitDirection::Horizontal => geometry.cols,
            SplitDirection::Vertical => geometry.cols / 2,
        }
        .max(1);

        let session_id = match self.session_manager.create_session(
            new_rows,
            new_cols,
            self.default_cwd.as_deref(),
        ) {
            Ok(session_id) => session_id,
            Err(error) => {
                tracing::error!(error = ?error, "failed to split session");
                return None;
            }
        };
        let tab = self.tabs[self.current_tab].as_mut()?;

        if !tab.split_active(direction, session_id) {
            return None;
        }

        self.wheel_remainder = 0.0;
        self.resize_tab();
        Some(session_id)
    }

    fn switch_tab(&mut self, tab: usize) -> bool {
        if tab >= self.tabs.len() {
            return false;
        }
        if self.tabs[tab].is_none() {
            let id = match self.session_manager.create_session(
                self.terminal_rows().max(1),
                self.cols.max(1),
                self.default_cwd.as_deref(),
            ) {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!(error = ?error, "failed to create tab session");
                    return false;
                }
            };
            self.tabs[tab] = Some(Tab::new(id));
        }

        self.current_tab = tab;
        self.wheel_remainder = 0.0;
        self.resize_tab();
        true
    }

    fn move_session_to_tab(&mut self, session: SessionId, target: usize) -> bool {
        if target >= self.tabs.len() || target == self.current_tab {
            return false;
        }
        let mut source = None;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if index == target {
                continue;
            }
            if let Some(tab) = tab.as_mut()
                && tab.remove_session(session)
            {
                source = Some(index);
                break;
            }
        }
        let Some(source) = source else {
            return false;
        };
        self.detached_sessions.retain(|id| *id != session);

        match self.tabs[target].as_mut() {
            Some(tab) => {
                if !tab.split_active(SplitDirection::Vertical, session) {
                    self.detached_sessions.push(session);
                }
            }
            None => self.tabs[target] = Some(Tab::new(session)),
        }

        self.current_tab = target;
        self.wheel_remainder = 0.0;
        if self.tabs[source]
            .as_ref()
            .is_some_and(|tab| !tab.is_empty())
        {
            self.resize_tab_at(source);
        }
        self.resize_tab();
        true
    }

    fn detach_active_session(&mut self) -> bool {
        let Some(active_session) = self.active_session() else {
            return false;
        };
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return false;
        };
        if !tab.remove_session(active_session) {
            return false;
        }
        self.detached_sessions.push(active_session);
        if self.tabs[self.current_tab]
            .as_ref()
            .is_some_and(Tab::is_empty)
        {
            self.tabs[self.current_tab] = None;
            self.switch_to_previous_live_tab_or_stay(self.current_tab);
        } else {
            self.resize_tab();
        }
        true
    }

    fn close_session(&mut self, session: SessionId) -> bool {
        if self.session_manager.session(session).is_none() {
            return false;
        }
        self.session_manager.remove_session(session);
        self.detached_sessions.retain(|detached| *detached != session);

        let mut removed_current_tab = false;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let Some(tab_state) = tab.as_mut() else {
                continue;
            };
            if !tab_state.remove_session(session) {
                continue;
            }
            if tab_state.is_empty() {
                *tab = None;
                removed_current_tab |= index == self.current_tab;
            }
        }

        if removed_current_tab {
            self.switch_to_previous_live_tab_or_stay(self.current_tab);
        } else {
            self.resize_tab();
        }
        true
    }

    fn reattach_session(&mut self, session: SessionId, target: usize) -> bool {
        if target >= self.tabs.len() || !self.detached_sessions.contains(&session) {
            return false;
        }
        self.detached_sessions.retain(|id| *id != session);
        match self.tabs[target].as_mut() {
            Some(tab) => {
                if !tab.split_active(SplitDirection::Vertical, session) {
                    self.detached_sessions.push(session);
                    return false;
                }
            }
            None => self.tabs[target] = Some(Tab::new(session)),
        }
        self.current_tab = target;
        self.wheel_remainder = 0.0;
        self.resize_tab();
        true
    }

    pub fn live_detached_sessions(&self) -> Vec<SessionId> {
        self.detached_sessions
            .iter()
            .copied()
            .filter(|id| self.session_manager.session(*id).is_some())
            .collect()
    }

    fn rename_session(&mut self, session: SessionId, name: &str) -> bool {
        let Some(session) = self.session_manager.session_mut(session) else {
            return false;
        };
        session.rename(name.to_string());
        true
    }

    fn rename_active(&mut self, name: &str) {
        let Some(session) = self.active_session() else {
            return;
        };
        self.rename_session(session, name);
    }

    fn terminal_rows(&self) -> u16 {
        if self.rows > Self::STATUS_BAR_ROWS && !self.status_bar_hidden {
            self.rows - Self::STATUS_BAR_ROWS
        } else {
            self.rows
        }
    }

    fn tab_program_name(&self, tab_index: usize) -> &str {
        self.tabs[tab_index]
            .as_ref()
            .and_then(Tab::active_session)
            .and_then(|session_id| self.session_manager.session(session_id))
            .map_or("shell", |session| session.title())
    }

    fn status_tabs(&self) -> Vec<(String, bool)> {
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| {
                tab.as_ref().map(|_| {
                    (
                        format!("[{}] {}", index + 1, self.tab_program_name(index)),
                        index == self.current_tab,
                    )
                })
            })
            .collect()
    }

    pub fn open_session_in_dir(&mut self, path: &Path) {
        let next_free = self.tabs.iter().position(|tab| tab.is_none());
        if let Some(free) = next_free {
            let id = match self.session_manager.create_session(
                self.terminal_rows().max(1),
                self.cols.max(1),
                Some(path),
            ) {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!(error = ?error, "failed to create session in dir");
                    return;
                }
            };
            self.tabs[free] = Some(Tab::new(id));
            self.current_tab = free;
            self.wheel_remainder = 0.0;
            self.resize_tab();
            return;
        }
        if self.tabs[self.current_tab].is_some() {
            let id = match self.session_manager.create_session(
                self.terminal_rows().max(1),
                self.cols.max(1),
                Some(path),
            ) {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!(error = ?error, "failed to create session in dir");
                    return;
                }
            };
            let Some(tab) = &mut self.tabs[self.current_tab] else {
                return;
            };
            tab.split_active(SplitDirection::Horizontal, id);
        }
    }

    pub fn create_remote_session(&mut self, session: &SshConnection) {
        let next_free = self.tabs.iter().position(|tab| tab.is_none());
        if let Some(free) = next_free {
            let id = match self.session_manager.create_remote_session(
                self.terminal_rows().max(1),
                self.cols.max(1),
                session,
            ) {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!(error = ?error, "failed to create tab session");
                    return;
                }
            };
            self.tabs[free] = Some(Tab::new(id));
            self.current_tab = free;
            self.wheel_remainder = 0.0;
            self.resize_tab();
            return;
        }
        if self.tabs[self.current_tab].is_some() {
            let id = match self.session_manager.create_remote_session(
                self.terminal_rows().max(1),
                self.cols.max(1),
                session,
            ) {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!(error = ?error, "failed to create tab session");
                    return;
                }
            };

            let Some(tab) = &mut self.tabs[self.current_tab] else {
                return;
            };

            tab.split_active(SplitDirection::Horizontal, id);
        }
    }

    /// Compose the status-bar view from a snapshot so UI chrome lives in `ui::status_bar`.
    fn render_status_bar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.status_bar.read(cx).render(window, cx)
    }
}
*/

impl Focusable for Surface {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Surface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();

        // Sync snapshots owned elsewhere into the bar (single owner for
        // display, source data stays in workspace/terminal/surface).
        let tabs = self.workspace.status_tabs();
        let ssh = self.workspace.ssh_sessions.clone();
        let line_h = self.terminal.line_height;
        let callbacks = self.registered_callbacks.clone();
        let bar_height = self.terminal.status_height;
        let bar_hidden = self.terminal.status_hidden;
        {
            let bar = self.status_bar.clone();
            bar.update(cx, |bar, cx| {
                bar.set_tabs(tabs);
                bar.set_ssh_sessions(ssh);
                bar.set_registered_callbacks(callbacks);
                bar.set_line_height(line_h);
                bar.set_status_height(bar_height);
                bar.set_status_bar_hidden(bar_hidden);
                cx.notify();
            });
        }

        // Drain Lua-queued UI intents through the same GPUI actions the
        // key paths dispatch directly — one shared set of `.on_action`
        // handlers below performs the work with window and cx in scope.
        // (`window.dispatch_action` defers via `cx.defer`, like the
        // key-event flow, so dialogs never open mid-render.)
        for action in std::mem::take(&mut self.pending_actions) {
            window.dispatch_action(action, cx);
        }

        let bounds_entity = cx.entity();
        let ime_entity = bounds_entity.clone();
        let status_bar = self.status_bar.clone();
        v_flex()
            .id("main")
            .bg(theme::role::window_bg())
            .track_focus(&self.focus_handle)
            .key_context("terminal")
            .on_action(cx.listener(|_, action: &OpenOverlay, window, cx| {
                let entity = cx.entity();
                Overlay::open(action.screen, entity, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenCommandPrompt, _, cx| {
                this.open_status_prompt(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenLuaPrompt, _, cx| {
                this.open_lua_prompt(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, action: &OpenRenamePrompt, _, cx| {
                this.open_rename_prompt_with(cx, action.name.clone());
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleStatusBar, _, cx| {
                this.toggle_status_bar(cx);
            }))
            .on_action(cx.listener(|this, action: &ShowStatusMessage, _, cx| {
                let text = action.text.clone();
                this.status_bar.update(cx, |bar, cx| {
                    bar.show_status_message(text, cx);
                });
            }))
            .on_action(cx.listener(|this, _: &ToggleFullscreen, window, cx| {
                this.fullscreen = !this.fullscreen;
                window.toggle_fullscreen();
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let entity = cx.entity();
                this.handle_key_down(event, entity, window, cx);
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                this.handle_key_up(event, cx);
            }))
            .p_2()
            .text_color(gpui::white())
            .size_full()
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.handle_mouse_down(event, window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.handle_mouse_down(event, window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.handle_mouse_down(event, window, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseUpEvent, _, cx| {
                            this.handle_mouse_up(event, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(|this, event: &MouseUpEvent, _, cx| {
                            this.handle_mouse_up(event, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseUpEvent, _, cx| {
                            this.handle_mouse_up(event, cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        this.handle_mouse_move(event, cx);
                    }))
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                        this.handle_scroll(event, cx);
                    }))
                    .on_children_prepainted(move |bounds, _, cx| {
                        let next = bounds.into_iter().next();
                        bounds_entity.update(cx, |this, cx| {
                            if this.terminal.terminal_bounds != next {
                                this.terminal.terminal_bounds = next;
                                cx.notify();
                            }
                        });
                    })
                    .child(div().size_full().child(
                        container_query(cx.processor(
                            |this, content_size: gpui::Size<Pixels>, window, _| {
                                this.sync_surface(window, content_size);
                                this.paint_terminal();
                                div()
                                    .size_full()
                                    .children(this.terminal.target.as_ref().map(|target| {
                                        target.surface().object_fit(gpui::ObjectFit::Fill).size_full()
                                    }))
                            },
                        )),
                    ))
                    // Paint-phase hook that registers the terminal IME input
                    // handler while the terminal is focused. Paints nothing and
                    // handles no input itself, so mouse events keep bubbling to
                    // the handlers above.
                    .child(
                        canvas(
                            |_, _, _| (),
                            {
                                let entity = ime_entity.clone();
                                let focus = self.focus_handle.clone();
                                move |bounds, _, window: &mut Window, cx: &mut App| {
                                    window.handle_input(
                                        &focus,
                                        ElementInputHandler::new(bounds, entity.clone()),
                                        cx,
                                    );
                                }
                            },
                        )
                        .absolute()
                        .left_0()
                        .top_0()
                        .size_full(),
                    ),
            )
            .child(status_bar)
    }
}

pub trait ResultExt {
    type OK;

    #[track_caller]
    fn log(self) -> Self;
    #[track_caller]
    fn into_log(self)
    where
        Self: Sized,
    {
        _ = self.log();
    }
    #[track_caller]
    fn log_assert(self) -> Self::OK;
}

impl<T, E> ResultExt for std::result::Result<T, E>
where
    E: Display,
{
    type OK = T;

    #[track_caller]
    fn log(self) -> Self {
        if let Err(error) = &self {
            let caller = Location::caller();

            tracing::error!(
                error = %error,
                caller.file = caller.file(),
                caller.line = caller.line(),
                caller.column = caller.column(),
                "Result contained an error"
            );
        }

        self
    }

    #[track_caller]
    fn log_assert(self) -> Self::OK {
        match self.log() {
            Ok(value) => value,
            Err(error) => {
                panic!("ResultExt::log_assert failed: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::status_bar::bar_height_px;

    #[test]
    fn status_bar_height_scales_with_line_height() {
        assert_eq!(bar_height_px(1.0, 28.0), 28.0);
        assert_eq!(bar_height_px(2.0, 26.4), 52.8);
        // Floored so degenerate configs stay visible.
        assert_eq!(bar_height_px(0.0, 28.0), 14.0);
        assert_eq!(bar_height_px(-3.0, 28.0), 14.0);
    }
}




/// Key dispatch tests for the status prompts. These drive the real window
/// key dispatch (keymap → focus → listeners), unlike unit tests that call
/// handlers directly: trigger keys must not leak, Tab must complete, and
/// Up/Down must navigate the command menu.
#[cfg(test)]
mod prompt_key_tests {
    use super::*;

    #[gpui::test]
    async fn lua_trigger_does_not_leak(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        let (view, vcx) =
            cx.add_window_view(|window, cx| Surface::new(window, cx, None, lua, tx, rx));
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
            });
        });
        vcx.simulate_keystrokes("ctrl-b");
        vcx.update(|_, cx| {
            assert!(view.read(cx).input.action_mode);
        });
        vcx.simulate_keystrokes("l");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        vcx.update(|_, cx| {
            let surface = view.read(cx);
            assert_eq!(
                surface.status_bar.read(cx).prompt_mode(),
                Some(Mode::Lua)
            );
            assert_eq!(
                surface
                    .status_bar
                    .read(cx)
                    .lua_editor()
                    .read(cx)
                    .value(cx)
                    .to_string(),
                "",
                "trigger key leaked into the Lua prompt"
            );
        });
    }

    #[gpui::test]
    async fn command_trigger_does_not_leak(cx: &mut gpui::TestAppContext) {        cx.update(gpui_component::init);
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        let (view, vcx) =
            cx.add_window_view(|window, cx| Surface::new(window, cx, None, lua, tx, rx));
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
            });
        });
        vcx.simulate_keystrokes("ctrl-b");
        vcx.simulate_keystrokes("semicolon");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        vcx.update(|_, cx| {
            let surface = view.read(cx);
            assert_eq!(
                surface.status_bar.read(cx).prompt_mode(),
                Some(Mode::Command)
            );
            let buffer = surface.status_bar.read(cx).command_value(cx);
            assert_eq!(buffer, "", "trigger key leaked into the command prompt");
        });
    }
}

    /// The text phase duplicates KeyDown delivery: plain text without an IME
    /// composition in flight must be ignored (the KeyDown path sent it),
    /// while a real commit still clears preedit state.
    #[gpui::test]
    async fn text_phase_ignores_non_ime_text(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        let (view, vcx) =
            cx.add_window_view(|window, cx| Surface::new(window, cx, None, lua, tx, rx));
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
            });
        });
        vcx.simulate_keystrokes("ctrl-b");
        vcx.simulate_keystrokes("semicolon");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        // Plain text with no composition in flight: prompt buffer untouched.
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                surface.replace_text_in_range(None, "x", window, cx);
            });
        });
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            let buffer = surface.status_bar.read(cx).command_value(cx);
            assert_eq!(buffer, "", "text phase duplicated KeyDown insertion");
            assert!(surface.terminal.ime_preedit.is_none());
        });

        // An IME commit clears the staged preedit.
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                surface.terminal.ime_preedit = Some("ni".to_string());
                surface.replace_text_in_range(None, "ni", window, cx);
            });
        });
        vcx.update(|_, cx| {
            assert!(view.read(cx).terminal.ime_preedit.is_none());
        });
    }


/// The `status_height` Lua option reaches the surface (applied + clamped).
#[cfg(test)]
mod status_height_tests {
    use super::*;

    fn surface_with(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<Surface>, &mut gpui::VisualTestContext) {
        cx.update(gpui_component::init);
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        let (view, vcx) =
            cx.add_window_view(|window, cx| Surface::new(window, cx, None, lua, tx, rx));
        (view, vcx)
    }

    #[gpui::test]
    async fn status_height_defaults_and_applies(cx: &mut gpui::TestAppContext) {
        let (view, _) = surface_with(cx);
        view.update(cx, |surface, _| {
            assert_eq!(surface.terminal.status_height, 1.0);
            let lua = surface.lua.clone();
            config::with_env(surface, |_| {
                lua.load("py:config({ status_height = 2.5 })").exec()
            })
            .unwrap();
            assert_eq!(surface.terminal.status_height, 2.5);
        });
    }

    #[gpui::test]
    async fn status_height_clamps_degenerate_values(cx: &mut gpui::TestAppContext) {
        let (view, _) = surface_with(cx);
        view.update(cx, |surface, _| {
            let lua = surface.lua.clone();
            config::with_env(surface, |_| {
                lua.load("py:config({ status_height = 0 })").exec()
            })
            .unwrap();
            assert_eq!(surface.terminal.status_height, 0.5);
        });
    }
}

#[cfg(test)]
mod prompt_completion_tests {
    use super::*;

    #[gpui::test]
    async fn command_tab_accepts_highlight(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        let (view, vcx) =
            cx.add_window_view(|window, cx| Surface::new(window, cx, None, lua, tx, rx));
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
            });
        });
        vcx.simulate_keystrokes("ctrl-b");
        vcx.simulate_keystrokes("semicolon");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("s w");
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            let buffer = surface.status_bar.read(cx).command_value(cx);
            assert_eq!(buffer, "sw");
        });
        vcx.simulate_keystrokes("tab");
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            let buffer = surface.status_bar.read(cx).command_value(cx);
            assert_eq!(buffer, "switch", "tab should accept the first match");
        });
    }

    #[gpui::test]
    async fn command_arrows_navigate_menu(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        let (view, vcx) =
            cx.add_window_view(|window, cx| Surface::new(window, cx, None, lua, tx, rx));
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
            });
        });
        vcx.simulate_keystrokes("ctrl-b");
        vcx.simulate_keystrokes("semicolon");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("s");
        vcx.update(|_, cx| {
            assert_eq!(
                view.read(cx).status_bar.read(cx).completion_selected(),
                0
            );
        });
        vcx.simulate_keystrokes("down");
        vcx.update(|_, cx| {
            assert_eq!(
                view.read(cx).status_bar.read(cx).completion_selected(),
                1
            );
        });
        vcx.simulate_keystrokes("up");
        vcx.update(|_, cx| {
            assert_eq!(
                view.read(cx).status_bar.read(cx).completion_selected(),
                0
            );
        });
        // Tab accepts the highlighted (second) match, not just the first.
        vcx.simulate_keystrokes("down");
        vcx.simulate_keystrokes("tab");
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            let expected = overlay::filter_commands(
                surface.status_bar.read(cx).palette_commands(),
                "s",
            )
            .into_iter()
            .nth(1)
            .map(|cmd| cmd.name);
            let buffer = surface.status_bar.read(cx).command_value(cx);
            assert_eq!(buffer, expected.expect("second match exists"));
        });
    }

    #[gpui::test]
    async fn lua_tab_accepts_first_match(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        let (view, vcx) =
            cx.add_window_view(|window, cx| Surface::new(window, cx, None, lua, tx, rx));
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
            });
        });
        vcx.simulate_keystrokes("ctrl-b");
        vcx.simulate_keystrokes("l");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        // Type into the focused editor through real dispatch.
        vcx.simulate_keystrokes("p r i");
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            assert_eq!(
                surface
                    .status_bar
                    .read(cx)
                    .lua_editor()
                    .read(cx)
                    .value(cx)
                    .to_string(),
                "pri".to_string(),
                "typing must reach the focused Lua editor"
            );
        });
        vcx.simulate_keystrokes("tab");
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            assert_eq!(
                surface
                    .status_bar
                    .read(cx)
                    .lua_editor()
                    .read(cx)
                    .value(cx)
                    .to_string(),
                "print".to_string(),
                "tab should accept the first Lua match"
            );
        });
    }

    #[gpui::test]
    async fn command_enter_unknown_shows_message(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        let (view, vcx) =
            cx.add_window_view(|window, cx| Surface::new(window, cx, None, lua, tx, rx));
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
            });
        });
        vcx.simulate_keystrokes("ctrl-b");
        vcx.simulate_keystrokes("semicolon");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("z z z");
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            assert_eq!(surface.status_bar.read(cx).prompt_mode(), None);
            assert_eq!(
                surface.status_bar.read(cx).message().as_deref(),
                Some("unknown command: zzz")
            );
        });
    }

    #[gpui::test]
    async fn rename_enter_closes_prompt(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        let (view, vcx) =
            cx.add_window_view(|window, cx| Surface::new(window, cx, None, lua, tx, rx));
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
                surface.open_rename_prompt_with(cx, "old-name".to_string());
            });
        });
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            assert_eq!(
                surface.status_bar.read(cx).prompt_mode(),
                Some(Mode::Rename)
            );
            assert_eq!(
                surface.status_bar.read(cx).rename_value(cx),
                "old-name"
            );
        });
        // Prefill is selected: typing replaces it.
        vcx.simulate_keystrokes("n e w");
        vcx.simulate_keystrokes("enter");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            assert_eq!(surface.status_bar.read(cx).prompt_mode(), None);
        });
    }
}
