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
mod lua_input;
#[cfg(feature = "install")]
mod logging;
mod overlay;
mod pty;
mod renderer;
mod status;
mod terminal;

use std::{array, collections::HashMap, fmt::Display, mem, ops::Range, panic::Location, path::{Path, PathBuf}, sync::Arc};
#[cfg(not(feature = "install"))]
use tracing_subscriber::prelude::*;

use anyhow::Result;
use clap::Parser;
use gpui::{
    Anchor, App, Bounds, ClipboardItem, Context, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, IntoElement, KeyDownEvent, KeyUpEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, Subscription, TextInputConfiguration, UTF16Selection, Window, actions, anchored, canvas, container_query, deferred, div, point, prelude::*, px, size
};
use gpui_component::{ActiveTheme as _, Root, Theme, ThemeMode, h_flex, v_flex};
use gpui_wgpu::{WgpuContextHandle, WgpuRenderTarget};
use mlua::{
    Lua, LuaOptions, StdLib,
    prelude::{LuaFunction, LuaMultiValue, LuaTable},
};

use crate::{
    config::KeyBinding,
    lua_input::{
        COMPLETION_MENU_LIMIT, LuaInputEvent, LuaSingleLineInput, lua_match_labels,
        split_completion_target,
    },
    overlay::{Cmd, LuaAction, Screen},
    pty::{Event as PtyEvent, SshConnection},
    renderer::{ImePreedit, Pane, Renderer},
    status::{Mode, StatusBar},
    terminal::{
        Divider, PaneGeometry, PanePathStep, SessionId, SessionManager, SplitDirection, Tab,
        TerminalSession,
    },
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

#[derive(clap::Parser)]
struct Cli {
    path: Option<PathBuf>,
}

pub struct Surface {
    pub focus_handle: FocusHandle,
    context: Option<WgpuContextHandle>,
    target: Option<WgpuRenderTarget>,
    pub renderer: Option<Renderer>,
    terminal_bounds: Option<Bounds<Pixels>>,
    terminal_scale: f32,

    pub session_manager: SessionManager,
    pub event_tx: async_channel::Sender<PtyEvent>,

    pub tabs: [Option<Tab>; 9],
    pub current_tab: usize,
    pub detached_sessions: Vec<SessionId>,

    pub rows: u16,
    pub cols: u16,
    pub font_size: f32,
    pub line_height: f32,
    pub font_family: Option<String>,

    pub status_bar_hidden: bool,
    pub status: StatusBar,
    /// Status bar height as a multiple of `line_height` (1.0 = one row).
    /// Configurable via `status_height`; every bar variant (tabs, prompts,
    /// Lua editor) pins to it so the bar never changes size between modes.
    pub status_height: f32,
    /// Lua status-prompt input (EditorState with mlua-backed completion).
    /// Lives as its own entity; only rendered/focused while a Lua prompt is
    /// open. Shares the app Lua state, but Enter evaluation stays with the
    /// host (`SubmitRequested`) so `py` and history semantics are unchanged.
    lua_editor: Entity<LuaSingleLineInput>,
    /// Set when a Lua prompt opens without a window at hand (Lua API); the
    /// next render focuses the editor.
    lua_prompt_needs_focus: bool,
    _subscriptions: Vec<Subscription>,

    pub ssh_sessions: Vec<SshConnection>,
    pub keymap: HashMap<KeyBinding, KeyAction>,
    pub action: KeyBinding,
    pub action_mode: bool,
    pub pending_move_to_tab: bool,
    pub resize_mode_held: bool,
    pub resize_mode_used: bool,
    pub wheel_remainder: f32,
    divider_drag: Option<DividerDrag>,
    ime_preedit: Option<String>,

    pub registered_callbacks: Vec<LuaAction>,
    pub palette_commands: Vec<Cmd>,
    pub fullscreen: bool,
    pub pending_fullscreen_toggle: bool,
    pub pending_overlay: Option<Screen>,
    pub default_cwd: Option<PathBuf>,

    pub releases: Vec<self_update::Release>,
    pub releases_loading: bool,

    pub lua: Lua,
    initial_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct DividerDrag {
    path: Vec<PanePathStep>,
    direction: SplitDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinAction {    Action,
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
                        // Config loads after the window exists, so a
                        // `fullscreen = true` config cannot shape the initial
                        // bounds. Apply it on the first frame instead; the
                        // render loop already fulfills pending toggles.
                        if surface.fullscreen {
                            surface.pending_fullscreen_toggle = true;
                        }
                    });

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
    const STATUS_BAR_ROWS: u16 = 0;

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
        let session_manager = SessionManager::new(event_tx.clone());
        let mut surface = Self {
            focus_handle,
            context: None,
            target: None,
            renderer: None,
            terminal_bounds: None,
            terminal_scale: 1.0,
            session_manager,
            event_tx,
            tabs: array::from_fn(|_| None),
            current_tab: 0,
            detached_sessions: vec![],
            rows: 20,
            cols: 80,
            font_size: 24.0,
            line_height: 28.0,
            font_family: None,
            status_bar_hidden: false,
            status: StatusBar::new(),
            status_height: 1.0,
            lua_editor: cx.new(|cx| {
                LuaSingleLineInput::new_with_lua(lua.clone(), window, cx)
            }),
            lua_prompt_needs_focus: false,
            _subscriptions: Vec::new(),
            ssh_sessions: vec![],
            keymap: Self::default_keymap(),
            action: KeyBinding::control("b"),
            action_mode: false,
            pending_move_to_tab: false,
            resize_mode_held: false,
            resize_mode_used: false,
            wheel_remainder: 0.0,
            divider_drag: None,
            ime_preedit: None,
            registered_callbacks: vec![],
            palette_commands: vec![],
            fullscreen: false,
            pending_fullscreen_toggle: false,
            pending_overlay: None,
            default_cwd: None,
            releases: vec![],
            releases_loading: false,
            lua,
            initial_path,
        };
        surface.palette_commands = overlay::commands_for(&surface);
        // One menu design for both prompt modes: the editor's built-in
        // completion popover stays off (hover and diagnostics keep working);
        // the status bar renders the shared hand-rolled menu instead.
        surface.lua_editor.update(cx, |editor, cx| {
            editor.disable_builtin_completion(cx);
        });
        // Lua-prompt submits: evaluation stays with the host (history, `py`
        // environment, messages), so the editor only reports the line.
        {
            let editor = surface.lua_editor.clone();
            let subscription = cx.subscribe_in(
                &editor,
                window,
                |surface: &mut Surface,
                 _: &Entity<LuaSingleLineInput>,
                 event: &LuaInputEvent,
                 window: &mut Window,
                 cx: &mut Context<Surface>| {
                    if let LuaInputEvent::SubmitRequested(code) = event {
                        let code = code.clone();
                        surface.submit_lua_from_editor(&code, window, cx);
                    }
                },
            );
            surface._subscriptions.push(subscription);
        }

        // Lua-prompt keys, scoped to the `status-lua` wrapper context so they
        // only fire while the prompt editor is focused. Up/Down/Enter/Escape
        // stay with the editor's own (deeper) `Input` context.
        cx.bind_keys([
            gpui::KeyBinding::new("tab", LuaTabAccept, Some("Input")),
            gpui::KeyBinding::new("escape", LuaCancelPrompt, Some("status-lua")),
            gpui::KeyBinding::new("ctrl-p", LuaHistoryPrev, Some("status-lua")),
            gpui::KeyBinding::new("ctrl-n", LuaHistoryNext, Some("status-lua")),
        ]);
        // `Root` binds Tab/Shift-Tab to focus navigation; without these
        // deeper-context `NoAction` shadows the terminal would lose Tab to
        // focus jumps and the status Tab completion would never fire. A
        // matched `NoAction` dispatches nothing, so the keystroke keeps
        // falling through to `handle_key_down` (and, with the editor
        // focused, to its own `Input`-context bindings first).
        cx.bind_keys([
            gpui::KeyBinding::new("tab", gpui::NoAction {}, Some("terminal")),
            gpui::KeyBinding::new("shift-tab", gpui::NoAction {}, Some("terminal")),
        ]);

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
        if self.tabs.iter().any(|tab| tab.is_some()) {
            return;
        }
        let cwd = self.initial_path.as_deref().or(self.default_cwd.as_deref());
        if let Ok(id) = self.session_manager.create_session(20, 80, cwd) {
            self.tabs[0] = Some(Tab::new(id));
        }
    }

    /// Kick off a releases fetch on first open. Plain field writes — no
    /// entity access — so this is safe from key handling (leased) and render.
    /// Results arrive as `PtyEvent::ReleasesReady` on the background channel.
    pub fn ensure_releases_fetch(&mut self) {
        if self.releases.is_empty() && !self.releases_loading {
            self.releases_loading = true;
            crate::overlay::fetch_releases_async(self.event_tx.clone());
        }
    }

    /// Returns `true` when the app should stop listening (quit requested).
    fn handle_pty_event(&mut self, event: PtyEvent, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        match event {
            PtyEvent::Closed(id) => {
                if self.close_session(id) && self.session_manager.is_empty() {
                    cx.quit();
                    return true;
                }
                cx.notify();
            }
            PtyEvent::Data(id, data) => {
                self.session_manager.update_session(id, &data);
                cx.notify();
            }
            PtyEvent::ProgramChanged((id, title)) => {
                if let Some(session) = self.session_manager.session_mut(id) {
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
                self.releases = releases;
                self.releases_loading = false;
                cx.notify();
            }
            PtyEvent::Exit => {
                cx.quit();
                return true;
            }
        }
        // Expired messages clear lazily; the timer already scheduled a notify.
        if self.status.clear_message_if_expired() {
            cx.notify();
        }
        false
    }

    pub fn default_keymap() -> HashMap<KeyBinding, KeyAction> {
        use BuiltinAction as B;
        let mut keymap = HashMap::new();
        keymap.insert(KeyBinding::control("b"), KeyAction::Builtin(B::Action));
        keymap.insert(
            KeyBinding::new(
                Modifiers {
                    control: true,
                    shift: true,
                    ..Default::default()
                },
                "f",
            ),
            KeyAction::Builtin(B::Palette),
        );
        keymap.insert(
            KeyBinding::new(
                Modifiers {
                    control: true,
                    shift: true,
                    ..Default::default()
                },
                "s",
            ),
            KeyAction::Builtin(B::Sessions),
        );
        let action_keys: &[(&str, BuiltinAction)] = &[
            ("1", B::Tab(0)),
            ("2", B::Tab(1)),
            ("3", B::Tab(2)),
            ("4", B::Tab(3)),
            ("5", B::Tab(4)),
            ("6", B::Tab(5)),
            ("7", B::Tab(6)),
            ("8", B::Tab(7)),
            ("9", B::Tab(8)),
            ("k", B::NextTab),
            ("j", B::PrevTab),
            ("w", B::NextPane),
            ("v", B::SplitVertical),
            ("h", B::SplitHorizontal),
            ("t", B::ToggleDecorations),
            ("s", B::ToggleStatusBar),
            ("p", B::Palette),
            ("semicolon", B::StatusPrompt),
            ("l", B::LuaPrompt),
            ("m", B::MoveToTab),
            ("d", B::DetachSession),
            ("r", B::RenameSession),
            ("a", B::Detached),
        ];
        for (key, action) in action_keys {
            keymap.insert(
                KeyBinding::new(Modifiers::default(), *key),
                KeyAction::Builtin(*action),
            );
        }
        keymap
    }

    /// Queue an overlay dialog to open on the next frame. Lua callbacks and
    /// palette commands cannot reach the GPUI `Window` directly, so they stage
    /// the request here and `render` fulfills it (same pattern as the pending
    /// fullscreen toggle).
    pub fn request_overlay(&mut self, screen: Screen) {
        self.pending_overlay = Some(screen);
    }

    pub fn refresh_palette_commands(&mut self) {
        self.palette_commands = overlay::commands_for(self);
    }

    /// Show a transient status message with the standard 5s expiry.
    pub fn show_status_message(&mut self, text: String, cx: &mut Context<Self>) {
        self.status.show_message(text);
        cx.notify();
        Self::arm_message_timer(cx);
    }

    /// Clear an expired message on a timer, shared by every path that sets
    /// one (manual `show_message`, Lua evaluation, release selection).
    fn arm_message_timer(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            smol::Timer::after(status::MESSAGE_TIMEOUT).await;
            let _ = this.update(cx, |this: &mut Surface, cx| {
                if this.status.clear_message_if_expired() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Run a Lua-prompt line submitted from the editor: same history, `py`
    /// environment, and message semantics as the legacy Enter path, then
    /// clear the editor and hand focus back to the terminal.
    fn submit_lua_from_editor(&mut self, code: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut status = mem::take(&mut self.status);
        status.submit_lua(self, code);
        self.status = status;
        self.lua_editor.update(cx, |editor, cx| {
            editor.set_value("", window, cx);
        });
        window.focus(&self.focus_handle, cx);
        Self::arm_message_timer(cx);
        cx.notify();
    }

    /// Accept the first Lua completion: replace the unfinished name before
    /// the cursor, keep any trailing text. Same first-hit semantics as the
    /// command menu's Tab. Fires ahead of the component's own Tab binding;
    /// anything but our focused Lua editor propagates untouched.
    fn accept_lua_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.lua_editor.clone();
        if self.status.prompt_mode() != Some(Mode::Lua)
            || !editor.read(cx).is_focused(window, cx)
        {
            cx.propagate();
            return;
        }
        window.prevent_default();
        let value = editor.read(cx).value(cx).to_string();
        let offset = editor
            .read(cx)
            .editor()
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
        let (_, prefix) = split_completion_target(before);
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

    /// Cancel the Lua prompt: clear the editor and refocus the terminal.
    fn cancel_lua_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.prevent_default();
        cx.stop_propagation();
        self.status.cancel_prompt();
        self.lua_editor.update(cx, |editor, cx| {
            editor.set_value("", window, cx);
        });
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    /// Walk Lua history into the editor (Ctrl+P/Ctrl+N — Up/Down belong to
    /// the editor's own context and can't be rebound from outside it).
    fn lua_history_step(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        window.prevent_default();
        cx.stop_propagation();
        if let Some(text) = self.status.lua_history_step(back) {
            let editor = self.lua_editor.clone();
            editor.update(cx, |editor, cx| {
                editor.set_value(&text, window, cx);
            });
        }
        cx.notify();
    }

    fn dispatch_builtin(
        &mut self,
        action: BuiltinAction,
        entity: Entity<Surface>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            BuiltinAction::Action => {
                self.resize_mode_held = true;
                self.resize_mode_used = false;
                self.action_mode = true;
            }
            BuiltinAction::Palette => {
                let commands = self.palette_commands.clone();
                overlay::open_palette(commands, entity, window, cx);
            }
            BuiltinAction::Sessions => {
                let entries = overlay::session_entries(self);
                overlay::open_sessions(entries, entity, window, cx);
            }
            BuiltinAction::Tab(index) => {
                let _ = self.switch_tab(index);
            }
            BuiltinAction::NextTab => {
                let index = self.next_tab_index();
                let _ = self.switch_tab(index);
            }
            BuiltinAction::PrevTab => {
                let index = self.previous_tab_index();
                let _ = self.switch_tab(index);
            }
            BuiltinAction::NextPane => {
                let _ = self.focus_next_pane();
            }
            BuiltinAction::SplitVertical => {
                let _ = self.split_current_tab(SplitDirection::Vertical);
            }
            BuiltinAction::SplitHorizontal => {
                let _ = self.split_current_tab(SplitDirection::Horizontal);
            }
            BuiltinAction::ToggleDecorations => {
                // GPUI owns window chrome; see the matching Lua stub.
                self.status.show_message(
                    "window decorations toggle is not supported on GPUI".to_string(),
                );
            }
            BuiltinAction::ToggleStatusBar => {
                self.status_bar_hidden = !self.status_bar_hidden;
                self.resize_tab();
            }
            BuiltinAction::StatusPrompt => self.open_status_prompt(),
            BuiltinAction::LuaPrompt => self.open_lua_prompt(),
            BuiltinAction::MoveToTab => {
                self.pending_move_to_tab = true;
                self.action_mode = true;
            }
            BuiltinAction::DetachSession => {
                let _ = self.detach_active_session();
            }
            BuiltinAction::RenameSession => self.open_rename_prompt(),
            BuiltinAction::Detached => {
                let entries = overlay::detached_entries(self);
                let current_tab = self.current_tab;
                overlay::open_detached(entries, current_tab, entity, window, cx);
            }
        }
        cx.notify();
    }
}

impl Surface {
    fn tab_layouts(&self) -> Vec<(SessionId, PaneGeometry)> {
        let rows = self.terminal_rows();
        self.tabs[self.current_tab]
            .as_ref()
            .map(|tab| {
                tab.layout(PaneGeometry {
                    x: 0,
                    y: 0,
                    cols: self.cols,
                    rows,
                })
            })
            .unwrap_or_default()
    }

    fn tab_dividers(&self) -> Vec<Divider> {
        let rows = self.terminal_rows();
        self.tabs[self.current_tab]
            .as_ref()
            .map(|tab| {
                tab.dividers(PaneGeometry {
                    x: 0,
                    y: 0,
                    cols: self.cols,
                    rows,
                })
            })
            .unwrap_or_default()
    }

    fn clear_gpu_resources(&mut self) {
        self.context = None;
        self.target = None;
        self.renderer = None;
    }

    fn ensure_context(&mut self, window: &mut Window) -> Option<WgpuContextHandle> {
        let context = WgpuContextHandle::from_window(window)?;
        if self
            .context
            .as_ref()
            .is_some_and(|previous| !context.is_same_device(previous))
        {
            self.clear_gpu_resources();
        }
        if context.device_lost() {
            self.clear_gpu_resources();
            return None;
        }
        self.context = Some(context.clone());
        Some(context)
    }

    /// Size the offscreen target from the surface element's layout bounds
    /// (content box, logical pixels) instead of the full window viewport,
    /// so the texture maps 1:1 to where GPUI composites it.
    fn sync_surface(&mut self, window: &mut Window, content_size: gpui::Size<Pixels>) {
        let Some(context) = self.ensure_context(window) else {
            return;
        };
        let scale_factor = window.scale_factor();
        self.terminal_scale = scale_factor;
        let target_size = size(
            gpui::DevicePixels((f32::from(content_size.width) * scale_factor).round() as i32),
            gpui::DevicePixels((f32::from(content_size.height) * scale_factor).round() as i32),
        );
        let target_size = size(
            target_size.width.max(gpui::DevicePixels(1)),
            target_size.height.max(gpui::DevicePixels(1)),
        );

        match self.target.as_mut() {
            Some(target) => target.resize(&context, target_size),
            None => {
                self.target = Some(WgpuRenderTarget::new(&context, target_size));
            }
        }

        let Some(target) = self.target.as_ref() else {
            return;
        };

        if self.renderer.is_none() {
            let queue = context.queue().clone();
            let device = context.device().clone();
            self.renderer = Renderer::new(
                queue,
                device,
                target.format(),
                self.font_family.as_deref(),
                self.font_size,
                self.line_height,
            )
            .ok();
        }

        let cols = ((target_size.width.0 as f32 / (self.font_size / 2.0)) as u16).max(1);
        let rows = ((target_size.height.0 as f32 / self.line_height) as u16).max(1);
        if cols != self.cols || rows != self.rows {
            self.cols = cols;
            self.rows = rows;
            let layouts = self.tab_layouts();
            for (id, geometry) in layouts {
                self.session_manager
                    .resize_session(id, geometry.rows, geometry.cols);
            }
        }
    }

    fn paint_terminal(&mut self) {
        let Some(target) = self.target.as_ref() else {
            return;
        };
        let panes = self.tab_layouts();
        let active = self.active_session();
        let dividers = self.tab_dividers();
        // Computed before the renderer borrow below: `ime_preedit` takes
        // `&self` while the renderer needs `&mut self`.
        let ime_preedit = self.ime_preedit();

        let mut pane_data = Vec::with_capacity(panes.len());
        for (session_id, geometry) in panes {
            let Some(session) = self.session_manager.session(session_id) else {
                continue;
            };
            pane_data.push(Pane {
                screen: session.vt.screen(),
                cursor_style: &session.cursor_style,
                geometry,
                is_active: Some(session_id) == active,
            });
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let target_size = target.size();
        let size = [target_size.width.0 as u32, target_size.height.0 as u32];
        _ = renderer.render(
            target.view(),
            &pane_data,
            &dividers,
            ime_preedit.as_ref(),
            size,
        );
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

        // The status prompt owns the keyboard while active, as before —
        // except Lua mode, which is an EditorState with its own focus. If it
        // somehow isn't focused (just opened), focus it instead of running
        // the legacy buffer path.
        if self.status.is_active() {
            if self.status.prompt_mode() == Some(Mode::Lua) {
                let editor = self.lua_editor.clone();
                editor.update(cx, |editor, cx| editor.focus(window, cx));
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
                return;
            }
            let mut status = mem::take(&mut self.status);
            status.handle_key(self, &entity, event, window, cx);
            self.status = status;
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
            return;
        }

        // Resize mode: arrows resize the active split while the action key is
        // held (mirrors the previous `resize_mode_held` path).
        if self.resize_mode_held {
            let delta = match event.keystroke.key.as_str() {
                "left" => Some((SplitDirection::Vertical, -1)),
                "right" => Some((SplitDirection::Vertical, 1)),
                "up" => Some((SplitDirection::Horizontal, -1)),
                "down" => Some((SplitDirection::Horizontal, 1)),
                _ => None,
            };
            if let Some((direction, delta)) = delta {
                self.resize_mode_used = true;
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
        if no_mods && self.pending_move_to_tab {
            self.pending_move_to_tab = false;
            self.action_mode = false;
            if let Some(target) = digit_index(event)
                && let Some(session) = self.active_session()
            {
                self.move_session_to_tab(session, target);
            }
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
            return;
        }

        let matched = self
            .keymap
            .iter()
            .find_map(|(binding, action)| binding.matches_event(event).then(|| action.clone()));
        if let Some(matched_action) = matched {
            let was_in_action_mode = self.action_mode;
            let is_action_trigger = matches!(
                matched_action,
                KeyAction::Builtin(BuiltinAction::Action)
            );
            if was_in_action_mode && !is_action_trigger {
                self.action_mode = false;
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
        } else if self.action_mode {
            // Any unbound key leaves action mode, as before.
            self.action_mode = false;
            cx.notify();
        }

        let Some(active_session) = self.active_session() else {
            return;
        };
        let reset_scrollback = self
            .session_manager
            .session_mut(active_session)
            .is_some_and(TerminalSession::reset_scrollback);
        let consumed = self
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
        entity: Entity<Surface>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.status.is_active() {
            // Lua mode is editor-driven; repeats go to the focused editor
            // natively, never through the legacy buffer path.
            if self.status.prompt_mode() == Some(Mode::Lua) {
                window.prevent_default();
                cx.stop_propagation();
                return;
            }
            let mut status = mem::take(&mut self.status);
            status.handle_key(self, &entity, event, window, cx);
            self.status = status;
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // Resize repeats keep resizing while held.
        if self.resize_mode_held {
            let delta = match event.keystroke.key.as_str() {
                "left" => Some((SplitDirection::Vertical, -1)),
                "right" => Some((SplitDirection::Vertical, 1)),
                "up" => Some((SplitDirection::Horizontal, -1)),
                "down" => Some((SplitDirection::Horizontal, 1)),
                _ => None,
            };
            if let Some((direction, delta)) = delta {
                self.resize_mode_used = true;
                self.resize_active_pane(direction, delta);
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        if let Some(active_session) = self.active_session() {
            let consumed = self
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
        if event.keystroke.key.to_lowercase() == self.action.key {
            self.resize_mode_held = false;
            if self.resize_mode_used {
                self.action_mode = false;
            }
            self.resize_mode_used = false;
            cx.notify();
        }
    }

    /// Window-relative mouse position → logical pixels relative to the
    /// terminal texture origin.
    fn mouse_to_terminal(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.terminal_bounds?;
        Some((
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        ))
    }

    fn cell_metrics(&self) -> Option<(f32, f32)> {
        let scale = self.terminal_scale.max(f32::EPSILON);
        let cell_width = (self.font_size / 2.0) / scale;
        let line_height = self.line_height / scale;
        (cell_width > 0.0 && line_height > 0.0).then_some((cell_width, line_height))
    }

    /// Window-relative mouse position → 1-based terminal cell.
    fn mouse_to_cell(&self, position: Point<Pixels>) -> Option<(u16, u16)> {
        let (cell_width, line_height) = self.cell_metrics()?;
        let (dx, dy) = self.mouse_to_terminal(position)?;
        let col = ((dx.max(0.0) / cell_width).floor() as i32 + 1)
            .clamp(1, i32::from(self.cols.max(1))) as u16;
        let rows = self.terminal_rows().max(1);
        let row = ((dy.max(0.0) / line_height).floor() as i32 + 1)
            .clamp(1, i32::from(rows)) as u16;
        Some((col, row))
    }

    /// Logical-pixel position → fractional grid position, for divider drags.
    fn cursor_to_grid_position(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let (cell_width, line_height) = self.cell_metrics()?;
        Some(grid_position(
            x,
            y,
            cell_width,
            line_height,
            self.cols,
            self.terminal_rows(),
        ))
    }

    /// Divider under a terminal-relative logical-pixel position, with the
    /// same 6px slop as the previous implementation so thin dividers stay
    /// grabbable.
    fn divider_hit_test(&self, x: f32, y: f32) -> Option<Divider> {
        let (cell_width, line_height) = self.cell_metrics()?;
        hit_divider(&self.tab_dividers(), x, y, cell_width, line_height)
    }

    fn resize_dragged_divider(&mut self, drag: &DividerDrag, x: f32, y: f32) {
        let Some((col, row)) = self.cursor_to_grid_position(x, y) else {
            return;
        };
        let position = match drag.direction {
            SplitDirection::Vertical => col,
            SplitDirection::Horizontal => row,
        };
        let area = PaneGeometry {
            x: 0,
            y: 0,
            cols: self.cols,
            rows: self.terminal_rows(),
        };
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if !tab.resize_split_by_position(area, &drag.path, drag.direction, position) {
            return;
        }
        self.wheel_remainder = 0.0;
        self.resize_tab();
    }

    fn pane_at(&self, col: u16, row: u16) -> Option<(SessionId, u16, u16)> {
        for (session_id, geometry) in self.tab_layouts() {
            if !geometry.contains_global_cell(col, row) {
                continue;
            }
            let (col, row) = geometry.local_cell(col, row);
            return Some((session_id, col, row));
        }
        None
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
            let drag = DividerDrag {
                path: divider.path,
                direction: divider.direction,
            };
            self.resize_dragged_divider(&drag, x, y);
            self.divider_drag = Some(drag);
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
        if let Some(session) = self.session_manager.session_mut(session_id) {
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
        if self.divider_drag.take().is_some() {
            cx.notify();
            return;
        }
        let Some((col, row)) = self.mouse_to_cell(event.position) else {
            return;
        };
        let Some((session_id, col, row)) = self.pane_at(col, row) else {
            return;
        };
        if let Some(session) = self.session_manager.session_mut(session_id) {
            session.handle_mouse_up(event.button, &event.modifiers, col, row);
        }
        let _ = cx;
    }

    fn handle_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button.is_none() {
            return;
        }
        // An active divider drag owns the motion, like before.
        if let Some(drag) = self.divider_drag.clone()
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
        if let Some(session) = self.session_manager.session_mut(session_id) {
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
                let scale = self.terminal_scale.max(f32::EPSILON);
                f32::from(pixels.y) / (self.line_height / scale)
            }
        };
        let uses_local_scrollback = self
            .session_manager
            .session(session_id)
            .is_some_and(TerminalSession::uses_local_scrollback);
        let whole_lines = if uses_local_scrollback {
            self.take_wheel_steps(lines)
        } else {
            self.wheel_remainder = 0.0;
            0
        };
        if let Some(session) = self.session_manager.session_mut(session_id) {
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
        self.ime_preedit
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        // Repaints happen every frame; nothing to notify from `App`.
        self.ime_preedit = None;
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
        if self.ime_preedit.is_none() {
            return;
        }
        self.ime_preedit = None;
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
        self.ime_preedit = None;
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
        self.ime_preedit = (!new_text.is_empty()).then(|| new_text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (cell_width, line_height) = self.cell_metrics()?;
        let active_session = self.active_session()?;
        let session = self.session_manager.session(active_session)?;
        let (_, geometry) = self
            .tab_layouts()
            .into_iter()
            .find(|(session_id, _)| *session_id == active_session)?;
        let (row, col) = session.vt.screen().cursor_position();
        let x = element_bounds.origin.x
            + px(cell_width * (f32::from(geometry.x) + f32::from(col)));
        let y = element_bounds.origin.y
            + px(line_height * (f32::from(geometry.y) + f32::from(row)));
        Some(Bounds::new(point(x, y), size(px(cell_width), px(line_height))))
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

/// The shared status-bar completion menu: a constrained popover above the
/// bar, the `selected` hit highlighted (Tab accepts it). Used identically by
/// command mode (fuzzy command names) and Lua mode (semantic matches, always
/// the first hit — the editor owns Up/Down there) — one menu design, two
/// item sources.
fn completion_menu(items: &[String], selected: usize, cx: &mut App) -> impl IntoElement {
    if items.is_empty() {
        return div().into_any_element();
    }
    let selected = selected.min(items.len() - 1);
    deferred(anchored().anchor(Anchor::BottomLeft).child(div()
        .id("completion-menu")
        .max_w(px(480.))
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(gpui::rgb(0x313244))
        .bg(gpui::rgb(0x1e1e2e))
        .py_1()
        .px_1()
        .children(items.iter().enumerate().map(|(index, hint)| {
            div()
                .px_2()
                .rounded(cx.theme().radius)
                .when(index == selected, |this| {
                    this.bg(gpui::rgb(0xb4befe))
                        .text_color(gpui::rgb(0x1e1e2e))
                })
                .when(index != selected, |this| {
                    this.text_color(gpui::rgb(0xcdd6f4))
                })
                .child(hint.clone())
        }))))
        .priority_auto()
        .into_any_element()
}

impl Surface {
    /// Send committed text (IME commit or paste) to whoever owns input: the
    /// Lua editor, the legacy prompt, or the active pty.
    fn commit_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Lua prompt mode owns an editor: committed text appends there.
        if self.status.prompt_mode() == Some(Mode::Lua) {
            let editor = self.lua_editor.clone();
            let current = editor.read(cx).value(cx).to_string();
            editor.update(cx, |editor, cx| {
                editor.set_value(&format!("{current}{text}"), window, cx);
            });
            cx.notify();
            return;
        }
        if self.status.insert_text(text) {
            cx.notify();
            return;
        }
        let Some(active_session) = self.active_session() else {
            return;
        };
        let reset_scrollback = self
            .session_manager
            .session_mut(active_session)
            .is_some_and(TerminalSession::reset_scrollback);
        self.session_manager.send_text(active_session, text);
        if reset_scrollback {
            cx.notify();
        }
    }
}

/// Status bar height in logical pixels: `status_height` rows of
/// `line_height`. Every bar variant pins to it so mode switches never resize
/// the bar (and with it the terminal above).
fn status_bar_height_px(status_height: f32, line_height: f32) -> f32 {
    (status_height.max(0.5) * line_height).max(1.0)
}

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

    fn open_status_prompt(&mut self) {
        self.status_bar_hidden = false;
        self.status.open(Mode::Command);
        self.refresh_palette_commands();
        self.resize_tab();
    }

    fn open_rename_prompt(&mut self) {
        self.status_bar_hidden = false;
        let name = self
            .active_session()
            .and_then(|id| self.session_manager.session(id))
            .map(TerminalSession::title)
            .unwrap_or_default()
            .to_owned();
        self.status.open_rename(name);
        self.resize_tab();
    }

    fn open_lua_prompt(&mut self) {
        self.status_bar_hidden = false;
        self.status.open(Mode::Lua);
        // The editor is focused on the next frame (render owns a window;
        // Lua-API callers have none).
        self.lua_prompt_needs_focus = true;
        self.resize_tab();
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

    /// GPUI status bar: tab segments, or the active prompt, or a transient
    /// message. Same content model as the wgpu bar (tabs left, message right,
    /// prompt prefix + buffer + cursor), now as host UI instead of glyphs.
    /// Lua mode renders the `EditorState` input (completion, hover,
    /// diagnostics) instead of the legacy buffer.
    fn render_status_bar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.status_bar_hidden {
            return div().into_any_element();
        }
        // One height for every variant — tabs, legacy prompts, Lua editor —
        // so the bar never jumps between modes.
        let bar_h = px(status_bar_height_px(self.status_height, self.line_height));
        if self.status.prompt_mode() == Some(Mode::Lua) {
            let editor = self.lua_editor.clone();
            let matches = {
                let input = editor.read(cx);
                let value = input.value(cx).to_string();
                let offset = input.editor().read(cx).cursor().min(value.len());
                let before = value[..offset.min(value.len())].to_string();
                lua_match_labels(input.lsp().lua(), &before, COMPLETION_MENU_LIMIT)
            };
            return v_flex()
                .w_full()
                .text_sm()
                .child(completion_menu(&matches, 0, cx))
                .child(
                    div()
                        .id("status-lua")
                        .key_context("status-lua")
                        .on_action(cx.listener(
                            |this, _: &LuaTabAccept, window, cx| {
                                this.accept_lua_completion(window, cx);
                            },
                        ))
                        .on_action(cx.listener(
                            |this, _: &LuaCancelPrompt, window, cx| {
                                this.cancel_lua_prompt(window, cx);
                            },
                        ))
                        .on_action(cx.listener(
                            |this, _: &LuaHistoryPrev, window, cx| {
                                this.lua_history_step(true, window, cx);
                            },
                        ))
                        .on_action(cx.listener(
                            |this, _: &LuaHistoryNext, window, cx| {
                                this.lua_history_step(false, window, cx);
                            },
                        ))
                        .w_full()
                        .h(bar_h)
                        .flex()
                        .flex_col()
                        .justify_center()
                        .child(editor),
                )
                .into_any_element();
        }
        // Expired messages are dropped lazily; the timer in
        // `show_status_message` already scheduled a notify.
        let message = self.status.message().map(str::to_string);

        if let Some((mode, prefix, buffer, cursor_col)) = self.status.prompt_parts() {
            let cursor_byte = buffer
                .char_indices()
                .nth(cursor_col)
                .map(|(index, _)| index)
                .unwrap_or(buffer.len());
            let (before, rest) = buffer.split_at(cursor_byte.min(buffer.len()));
            // Command-mode completion menu: fuzzy matches for the name part,
            // first hit highlighted (Tab accepts it). A constrained popover
            // above the bar — deliberately not full width.
            let completions: Vec<String> = if mode == Mode::Command && !buffer.is_empty() {
                overlay::filter_commands(&self.palette_commands, buffer)
                    .into_iter()
                    .take(8)
                    .map(|cmd| cmd.hint())
                    .collect()
            } else {
                Vec::new()
            };
            // Input frame mirrors the Lua editor's frame; the cursor is a
            // normal thin bar one UI line tall instead of a reversed block.
            let cursor_h = window.line_height();
            return v_flex()
                .w_full()
                .text_sm()
                .child(completion_menu(&completions, self.status.completion_selected(), cx))
                .child(
                    h_flex()
                .w_full()
                .h(bar_h)
                .items_center()
                .gap_1()
                .px_1()
                .rounded(cx.theme().radius)
                .bg(cx.theme().input_background())
                .border_1()
                .border_color(cx.theme().ring)
                .text_color(gpui::rgb(0xcdd6f4))
                .child(
                    div()
                        .px_1()
                        .rounded(cx.theme().radius)
                        .bg(gpui::rgb(0xb4befe))
                        .text_color(gpui::rgb(0x1e1e2e))
                        .child(prefix.trim().to_string()),
                )
                .child(div().child(before.to_string()))
                .child(div().w_px().h(cursor_h).bg(gpui::rgb(0xcdd6f4)))
                .child(div().child(rest.to_string())),
                )
                .into_any_element();
        }

        let tabs = self.status_tabs();
        h_flex()
            .w_full()
            .text_sm()
            .px_1()
            .h(bar_h)
            .rounded(cx.theme().radius)
            .items_center()
            .bg(gpui::rgb(0x1e1e2e))
            .children(tabs.into_iter().map(|(label, is_active)| {
                div()
                    .px_2()
                    .py_0p5()
                    .mr_1()
                    .rounded(cx.theme().radius)
                    .when(is_active, |this| {
                        this.bg(gpui::rgb(0xb4befe))
                            .text_color(gpui::rgb(0x1e1e2e))
                    })
                    .when(!is_active, |this| {
                        this.bg(gpui::rgb(0x313244))
                            .text_color(gpui::rgb(0xcdd6f4))
                    })
                    .child(format!("{label}"))
            }))
            .when_some(message, |this, message| {
                this.child(
                    div()
                        .ml_auto()
                        .px_2()
                        .py_0p5()
                        .rounded(cx.theme().radius)
                        .bg(gpui::rgb(0xcba6f7))
                        .text_color(gpui::rgb(0x11111b))
                        .child(format!("{message}")),
                )
            })
            .into_any_element()
    }
}

impl Focusable for Surface {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Surface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.request_animation_frame();
        if self.status.clear_message_if_expired() {
            // Dropped during paint; no extra notify needed.
        }

        // Lua and palette commands stage overlay/fullscreen requests without a
        // window; fulfill them here where both are available.
        if self.pending_fullscreen_toggle {
            self.pending_fullscreen_toggle = false;
            window.toggle_fullscreen();
        }
        if let Some(screen) = self.pending_overlay.take() {
            let entity = cx.entity();
            // Fulfilled after render: the deferred frame holds no surface
            // lease, so the dialog builder may snapshot freely.
            window.defer(cx, move |window, cx| {
                entity.update(cx, |surface, cx| {
                    overlay::open_screen(screen, surface, entity.clone(), window, cx);
                });
            });
        }

        // Lua prompt focus handoff: Lua-API callers open prompts without a
        // window, so the first render after opening moves focus in.
        if self.lua_prompt_needs_focus && self.status.prompt_mode() == Some(Mode::Lua) {
            self.lua_prompt_needs_focus = false;
            let editor = self.lua_editor.clone();
            window.defer(cx, move |window, cx| {
                editor.update(cx, |editor, cx| editor.focus(window, cx));
            });
        }

        let bounds_entity = cx.entity();
        let ime_entity = bounds_entity.clone();
        v_flex()
            .id("main")
            .bg(gpui::rgba(0x181818FF))
            .track_focus(&self.focus_handle)
            .key_context("terminal")
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
                            if this.terminal_bounds != next {
                                this.terminal_bounds = next;
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
                                    .children(this.target.as_ref().map(|target| {
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
            .child(self.render_status_bar(window, cx))
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

    fn vertical_divider(x: u16, rows: u16) -> Divider {
        Divider {
            path: vec![PanePathStep::First],
            direction: SplitDirection::Vertical,
            x,
            y: 0,
            cols: 0,
            rows,
        }
    }

    fn horizontal_divider(y: u16, cols: u16) -> Divider {
        Divider {
            path: vec![PanePathStep::Second],
            direction: SplitDirection::Horizontal,
            x: 0,
            y,
            cols,
            rows: 0,
        }
    }

    #[test]
    fn divider_hit_test_finds_lines_within_slop() {
        let dividers = vec![vertical_divider(40, 24)];
        // cell 10px wide → line at x=400.
        assert!(hit_divider(&dividers, 400.0, 100.0, 10.0, 20.0).is_some());
        assert!(hit_divider(&dividers, 406.0, 100.0, 10.0, 20.0).is_some());
        assert!(hit_divider(&dividers, 394.0, 100.0, 10.0, 20.0).is_some());
        assert!(hit_divider(&dividers, 420.0, 100.0, 10.0, 20.0).is_none());
        // Past the divider's row span (+slop) misses.
        assert!(hit_divider(&dividers, 400.0, 24.0 * 20.0 + 7.0, 10.0, 20.0).is_none());

        let dividers = vec![horizontal_divider(12, 80)];
        // cell 20px tall → line at y=240.
        assert!(hit_divider(&dividers, 100.0, 240.0, 10.0, 20.0).is_some());
        assert!(hit_divider(&dividers, 100.0, 200.0, 10.0, 20.0).is_none());
        // Past the divider's column span (+slop) misses.
        assert!(hit_divider(&dividers, 80.0 * 10.0 + 7.0, 240.0, 10.0, 20.0).is_none());

        assert!(hit_divider(&[], 0.0, 0.0, 10.0, 20.0).is_none());
    }

    #[test]
    fn status_bar_height_scales_with_line_height() {
        assert_eq!(status_bar_height_px(1.0, 28.0), 28.0);
        assert_eq!(status_bar_height_px(2.0, 26.4), 52.8);
        // Floored so degenerate configs stay visible.
        assert_eq!(status_bar_height_px(0.0, 28.0), 14.0);
        assert_eq!(status_bar_height_px(-3.0, 28.0), 14.0);
    }

    #[test]
    fn grid_position_clamps_to_grid() {
        assert_eq!(grid_position(25.0, 30.0, 10.0, 20.0, 80, 24), (2.5, 1.5));
        assert_eq!(grid_position(-5.0, -5.0, 10.0, 20.0, 80, 24), (0.0, 0.0));
        assert_eq!(
            grid_position(10000.0, 10000.0, 10.0, 20.0, 80, 24),
            (80.0, 24.0)
        );
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
            assert!(view.read(cx).action_mode);
        });
        vcx.simulate_keystrokes("l");
        vcx.run_until_parked();
        vcx.update(|window, cx| {
            _ = window.draw(cx);
        });
        vcx.run_until_parked();

        vcx.update(|_, cx| {
            let surface = view.read(cx);
            assert_eq!(surface.status.prompt_mode(), Some(Mode::Lua));
            assert_eq!(
                surface.lua_editor.read(cx).value(cx).to_string(),
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
            assert_eq!(surface.status.prompt_mode(), Some(Mode::Command));
            let (_, _, buffer, _) = surface.status.prompt_parts().unwrap();
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

        // Plain text with no composition in flight: prompt buffer untouched.
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                surface.replace_text_in_range(None, "x", window, cx);
            });
        });
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            let (_, _, buffer, _) = surface.status.prompt_parts().unwrap();
            assert_eq!(buffer, "", "text phase duplicated KeyDown insertion");
            assert!(surface.ime_preedit.is_none());
        });

        // An IME commit clears the staged preedit.
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                surface.ime_preedit = Some("ni".to_string());
                surface.replace_text_in_range(None, "ni", window, cx);
            });
        });
        vcx.update(|_, cx| {
            assert!(view.read(cx).ime_preedit.is_none());
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
            assert_eq!(surface.status_height, 1.0);
            let lua = surface.lua.clone();
            config::with_env(surface, |_| {
                lua.load("py:config({ status_height = 2.5 })").exec()
            })
            .unwrap();
            assert_eq!(surface.status_height, 2.5);
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
            assert_eq!(surface.status_height, 0.5);
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
        let (view, mut vcx) =
            cx.add_window_view(|window, cx| Surface::new(window, cx, None, lua, tx, rx));
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
            });
        });
        vcx.simulate_keystrokes("ctrl-b");
        vcx.simulate_keystrokes("semicolon");
        vcx.simulate_keystrokes("s w");
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            let (_, _, buffer, _) = surface.status.prompt_parts().unwrap();
            assert_eq!(buffer, "sw");
        });
        vcx.simulate_keystrokes("tab");
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            let (_, _, buffer, _) = surface.status.prompt_parts().unwrap();
            assert_eq!(buffer, "switch", "tab should accept the first match");
        });
    }

    #[gpui::test]
    async fn command_arrows_navigate_menu(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        let (view, mut vcx) =
            cx.add_window_view(|window, cx| Surface::new(window, cx, None, lua, tx, rx));
        vcx.update(|window, cx| {
            view.update(cx, |surface, cx| {
                window.focus(&surface.focus_handle, cx);
            });
        });
        vcx.simulate_keystrokes("ctrl-b");
        vcx.simulate_keystrokes("semicolon");
        vcx.simulate_keystrokes("s");
        vcx.update(|_, cx| {
            assert_eq!(view.read(cx).status.completion_selected(), 0);
        });
        vcx.simulate_keystrokes("down");
        vcx.update(|_, cx| {
            assert_eq!(view.read(cx).status.completion_selected(), 1);
        });
        vcx.simulate_keystrokes("up");
        vcx.update(|_, cx| {
            assert_eq!(view.read(cx).status.completion_selected(), 0);
        });
        // Tab accepts the highlighted (second) match, not just the first.
        vcx.simulate_keystrokes("down");
        vcx.simulate_keystrokes("tab");
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            let expected = overlay::filter_commands(&surface.palette_commands, "s")
                .into_iter()
                .nth(1)
                .map(|cmd| cmd.name);
            let (_, _, buffer, _) = surface.status.prompt_parts().unwrap();
            assert_eq!(buffer, expected.expect("second match exists"));
        });
    }

    #[gpui::test]
    async fn lua_tab_accepts_first_match(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        let (view, mut vcx) =
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
                surface.lua_editor.read(cx).value(cx).to_string(),
                "pri".to_string(),
                "typing must reach the focused Lua editor"
            );
        });
        vcx.simulate_keystrokes("tab");
        vcx.update(|_, cx| {
            let surface = view.read(cx);
            assert_eq!(
                surface.lua_editor.read(cx).value(cx).to_string(),
                "print".to_string(),
                "tab should accept the first Lua match"
            );
        });
    }
}
