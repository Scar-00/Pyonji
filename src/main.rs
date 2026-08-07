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
#[cfg(feature = "install")]
mod logging;
mod overlay;
mod pty;
mod renderer;
mod terminal;
//mod util;

use mlua::{Lua, LuaOptions, StdLib, prelude::*};
use smol::Task;
#[cfg(not(feature = "install"))]
use tracing_subscriber::prelude::*;

use anyhow::{Context, Result};
use clap::Parser;
use pty::Event as PtyEvent;
use renderer::{ImePreedit, Pane, Renderer, StatusInput, StatusLine, StatusTab};
use std::{
    array,
    cell::RefCell,
    fmt::Display,
    panic::Location,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::error;
use winit::{
    application::ApplicationHandler,
    dpi::{PhysicalPosition, PhysicalSize},
    event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{Icon, Window, WindowId},
};

use crate::{
    config::KeyBinding,
    overlay::{LuaAction, Overlay, Screen},
    pty::SshConnection,
    terminal::{
        Divider, PaneGeometry, PanePathStep, SessionId, SessionManager, SplitDirection, Tab,
        TerminalSession,
    },
};

struct LocalExecutor {
    inner: smol::LocalExecutor<'static>,
    window: Option<Arc<Window>>,
}

impl LocalExecutor {
    pub fn new() -> Self {
        Self {
            inner: smol::LocalExecutor::new(),
            window: None,
        }
    }

    pub fn spawn<R: 'static>(&self, f: impl 'static + AsyncFn() -> R) -> Task<R> {
        let task = self.inner.spawn(async move { f().await });
        if let Some(window) = &self.window {
            window.request_redraw();
        };
        task
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn try_tick(&self) -> bool {
        self.inner.try_tick()
    }

    fn set_window(&mut self, window: Arc<Window>) {
        self.window = Some(window);
    }
}

#[derive(clap::Parser)]
struct Cli {
    path: Option<PathBuf>,
}

struct App {
    args: Cli,
    renderer: Option<Renderer>,
    pub window: Option<Arc<Window>>,
    session_manager: SessionManager,
    pub ssh_sessions: Vec<SshConnection>,
    modifiers: ModifiersState,
    line_height: f32,
    font_size: f32,
    font_family: Option<String>,
    rows: u16,
    cols: u16,
    tabs: [Option<Tab>; 9],
    action_mode: bool,
    pending_move_to_tab: bool,
    current_tab: usize,
    detached_sessions: Vec<SessionId>,
    cursor_pos: Option<(f64, f64)>,
    wheel_remainder: f32,
    divider_drag: Option<DividerDrag>,
    resize_mode_held: bool,
    resize_mode_used: bool,
    ime_enabled: bool,
    ime_preedit: Option<String>,
    status_bar_hidden: bool,
    status_prompt: Option<StatusPrompt>,
    status_message: Option<String>,
    _proxy: EventLoopProxy<PtyEvent>,

    pub local_executer: LocalExecutor,

    overlay: Option<Overlay>,

    action: KeyBinding,
    key_bindings: Vec<(KeyBinding, LuaFunction)>,
    registered_callbacks: Vec<LuaAction>,
    fullscreen: bool,
    default_cwd: Option<PathBuf>,

    lua: Lua,
}

#[derive(Clone, Copy)]
struct PaneHit {
    session_id: SessionId,
    col: u16,
    row: u16,
}

#[derive(Clone)]
struct DividerDrag {
    path: Vec<PanePathStep>,
    direction: SplitDirection,
}

enum StatusPromptMode {
    Command,
    Rename,
    Lua,
}

struct StatusPrompt {
    mode: StatusPromptMode,
    buffer: String,
    cursor: usize,
    history: Vec<String>,
    history_index: Option<usize>,
}

impl StatusPrompt {
    fn new(mode: StatusPromptMode) -> Self {
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

    let event_loop = EventLoop::<PtyEvent>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    let proxy = event_loop.create_proxy();

    config::watch(proxy.clone());

    let lua = unsafe { Lua::unsafe_new_with(StdLib::ALL_SAFE, LuaOptions::new()) };
    config::install_inspect(&lua)?;
    for module in config::LUA_MODULES {
        lua.load(*module).exec()?;
    }
    let mut app = App::new(cli, lua.clone(), proxy);

    config::load(&mut app);

    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app).map_err(|error| {
        error!(%error, "event loop failed");
        error.into()
    })
}

impl App {
    const TITLE: &str = cfg_select! {
        feature = "install" => "Pyonji",
        _ => {
            const_format::formatcp!("Pyonji {}", git_version::git_version!())
        },
    };
    const ICON: &[u8] = include_bytes!("../resources/icon.ico");
    const STATUS_BAR_ROWS: u16 = 1;
}

impl App {
    pub fn new(cli: Cli, lua: Lua, proxy: EventLoopProxy<PtyEvent>) -> Self {
        Self {
            args: cli,
            renderer: None,
            window: None,
            session_manager: SessionManager::new(proxy.clone()),
            ssh_sessions: vec![],
            modifiers: ModifiersState::default(),
            font_size: 24.0,
            line_height: 28.0,
            font_family: None,
            rows: 20,
            cols: 80,
            tabs: array::from_fn(|_| None),
            action_mode: false,
            pending_move_to_tab: false,
            current_tab: 0,
            detached_sessions: vec![],
            cursor_pos: None,
            wheel_remainder: 0.0,
            divider_drag: None,
            resize_mode_held: false,
            resize_mode_used: false,
            ime_enabled: false,
            ime_preedit: None,
            status_bar_hidden: false,
            status_prompt: None,
            status_message: None,
            _proxy: proxy,

            local_executer: LocalExecutor::new(),

            overlay: None,

            action: KeyBinding {
                mods: ModifiersState::CONTROL,
                key: KeyCode::KeyB,
            },
            key_bindings: vec![],
            registered_callbacks: vec![],
            fullscreen: false,
            default_cwd: None,

            lua,
        }
    }

    fn ui(&mut self) {
        let Some(mut overlay) = self.overlay.take() else {
            return;
        };
        if overlay.shown() {
            overlay.draw(self).expect("drawing overlay");
        }
        self.overlay = Some(overlay);
    }
}

impl ApplicationHandler<PtyEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let icon = image::load_from_memory(Self::ICON).ok().and_then(|image| {
            let data = image.to_rgba8().to_vec();
            Icon::from_rgba(data, image.width(), image.height()).ok()
        });
        let Ok(window) = event_loop.create_window(
            Window::default_attributes()
                .with_inner_size(PhysicalSize::new(1280, 720))
                .with_active(true)
                .with_window_icon(icon)
                .with_maximized(self.fullscreen)
                .with_title(Self::TITLE),
        ) else {
            event_loop.exit();
            return;
        };
        window.set_ime_allowed(true);
        let size = window.inner_size();
        self.rows = (size.height as f32 / self.line_height) as u16;
        self.cols = (size.width as f32 / (self.font_size / 2.0)) as u16;

        let window = Arc::new(window);
        self.renderer = match Renderer::new(
            window.clone(),
            self.font_family.as_deref(),
            self.font_size,
            self.line_height,
        ) {
            Ok(renderer) => Some(renderer),
            Err(error) => {
                error!(error = ?error, "failed to initialize renderer");
                None
            }
        };
        self.window = Some(window.clone());
        self.local_executer.set_window(window.clone());
        self.overlay = Overlay::new(self, size, self.font_size, self.line_height).ok();

        match self.session_manager.create_session(
            self.terminal_rows().max(1),
            self.cols.max(1),
            self.args.path.as_deref().or(self.default_cwd.as_deref()),
        ) {
            Ok(session) => {
                self.tabs[0] = Some(Tab::new(session));
                self.current_tab = 0;
                self.resize_tab();
            }
            Err(error) => error!(error = ?error, "failed to create initial session"),
        }
        self.update_ime_cursor_area();
        window.request_redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: PtyEvent) {
        match event {
            PtyEvent::Closed(id) => {
                self.session_manager.remove_session(id);
                self.detached_sessions.retain(|detached| *detached != id);
                if self.session_manager.is_empty() {
                    event_loop.exit();
                    return;
                }

                let mut removed_current_tab = false;
                for (index, tab) in self.tabs.iter_mut().enumerate() {
                    let Some(tab_state) = tab.as_mut() else {
                        continue;
                    };
                    if !tab_state.remove_session(id) {
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
                self.update_ime_cursor_area();
                self.request_redraw();
            }
            PtyEvent::Data(id, data) => {
                self.session_manager.update_session(id, &data);
                self.update_ime_cursor_area();
                self.request_redraw();
            }
            PtyEvent::ProgramChanged((id, title)) => {
                let Some(session) = self.session_manager.session_mut(id) else {
                    return;
                };
                session.set_title(title);
            }
            PtyEvent::ConfigChanged => {
                config::load(self);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::RedrawRequested => {
                //let start = std::time::Instant::now();
                let panes = self.tab_layouts();
                let active = self.active_session();
                let dividers = self.tab_dividers();
                let status_tabs = self.status_tabs();
                let status_input = self.status_input();
                let status_message = if self.status_bar_hidden {
                    None
                } else {
                    self.status_message.clone()
                };
                let status = if status_tabs.is_none() && status_input.is_none() && status_message.is_none()
                {
                    None
                } else {
                    Some(StatusLine {
                        tabs: status_tabs.as_deref(),
                        input: status_input.as_ref(),
                        message: status_message.as_deref(),
                    })
                };
                let ime_preedit = self.ime_preedit();
                self.ui();
                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };

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
                if let Err(e) = renderer.render(
                    &pane_data,
                    &dividers,
                    status,
                    ime_preedit.as_ref(),
                    self.overlay.as_ref(),
                ) {
                    error!(error = ?e, "failed to render");
                }
                if !self.local_executer.is_empty() {
                    self.local_executer.try_tick();
                    self.request_redraw();
                }
                //println!("render = {:?}", start.elapsed());
            }
            WindowEvent::Resized(size) => {
                if size.width == 0 || size.height == 0 {
                    return;
                }
                self.rows = (size.height as f32 / self.line_height) as u16;
                self.cols = (size.width as f32 / (self.font_size / 2.0)) as u16;
                self.resize_tab();
                let Some((window, renderer)) = self.window.as_ref().zip(self.renderer.as_mut())
                else {
                    return;
                };
                renderer.resize(size);
                window.request_redraw();
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.resize(size, self.font_size, self.line_height);
                }
                self.update_ime_cursor_area();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
                if !self.modifiers.control_key() {
                    self.resize_mode_held = false;
                    if self.resize_mode_used {
                        self.action_mode = false;
                    }
                    self.resize_mode_used = false;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = Some((position.x, position.y));
                if let Some(drag) = self.divider_drag.clone() {
                    self.resize_dragged_divider(&drag, position.x, position.y);
                    return;
                }
                let Some(hit) = self.pane_hit_test(position.x, position.y) else {
                    return;
                };
                if let Some(session) = self.session_manager.session_mut(hit.session_id) {
                    let reset_scrollback = if session.uses_local_scrollback() {
                        false
                    } else {
                        session.reset_scrollback()
                    };
                    session.handle_mouse_move(self.modifiers, hit.col, hit.row);
                    if reset_scrollback {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some((x, y)) = self.cursor_pos else {
                    return;
                };

                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            if let Some(divider) = self.divider_hit_test(x, y) {
                                let drag = DividerDrag {
                                    path: divider.path,
                                    direction: divider.direction,
                                };
                                self.resize_dragged_divider(&drag, x, y);
                                self.divider_drag = Some(drag);
                                return;
                            }
                        }
                        ElementState::Released if self.divider_drag.take().is_some() => {
                            return;
                        }
                        ElementState::Released => {}
                    }
                }

                let Some(hit) = self.pane_hit_test(x, y) else {
                    return;
                };

                if state == ElementState::Pressed {
                    self.set_active_session(hit.session_id);
                    self.update_ime_cursor_area();
                }

                if let Some(session) = self.session_manager.session_mut(hit.session_id) {
                    let reset_scrollback = if session.uses_local_scrollback() {
                        false
                    } else {
                        session.reset_scrollback()
                    };
                    session.handle_mouse_button(button, state, self.modifiers, hit.col, hit.row);
                    if reset_scrollback {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some((x, y)) = self.cursor_pos else {
                    return;
                };
                let Some(hit) = self.pane_hit_test(x, y) else {
                    return;
                };
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(pos) => {
                        if self.line_height > 0.0 {
                            (pos.y as f32) / self.line_height
                        } else {
                            0.0
                        }
                    }
                };
                let uses_local_scrollback = self
                    .session_manager
                    .session(hit.session_id)
                    .is_some_and(TerminalSession::uses_local_scrollback);
                let whole_lines = if uses_local_scrollback {
                    self.take_wheel_steps(lines)
                } else {
                    self.wheel_remainder = 0.0;
                    0
                };

                if let Some(session) = self.session_manager.session_mut(hit.session_id) {
                    if uses_local_scrollback {
                        if whole_lines != 0 && session.scroll_scrollback(whole_lines) {
                            self.request_redraw();
                        }
                    } else {
                        let reset_scrollback = session.reset_scrollback();
                        session.handle_mouse_wheel(lines, self.modifiers, hit.col, hit.row);
                        if reset_scrollback {
                            self.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(prompt) = self.status_prompt.take() {
                    self.handle_status_prompt(prompt, &event);
                    self.request_redraw();
                    return;
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    if self.key_bindings.is_empty() {
                        if code == KeyCode::KeyB && event.state == ElementState::Released {
                            self.resize_mode_held = false;
                            if self.resize_mode_used {
                                self.action_mode = false;
                            }
                            self.resize_mode_used = false;
                            return;
                        }
                        if event.state != ElementState::Pressed {
                            return;
                        }
                        if self.resize_mode_held {
                            match code {
                                KeyCode::ArrowLeft => {
                                    self.resize_mode_used = true;
                                    self.resize_active_pane(SplitDirection::Vertical, -1);
                                    return;
                                }
                                KeyCode::ArrowRight => {
                                    self.resize_mode_used = true;
                                    self.resize_active_pane(SplitDirection::Vertical, 1);
                                    return;
                                }
                                KeyCode::ArrowUp => {
                                    self.resize_mode_used = true;
                                    self.resize_active_pane(SplitDirection::Horizontal, -1);
                                    return;
                                }
                                KeyCode::ArrowDown => {
                                    self.resize_mode_used = true;
                                    self.resize_active_pane(SplitDirection::Horizontal, 1);
                                    return;
                                }
                                _ => {}
                            }
                        }
                        if self.modifiers.control_key() && matches!(code, KeyCode::KeyB) {
                            self.resize_mode_held = true;
                            self.resize_mode_used = false;
                            self.action_mode = true;
                            return;
                        } else if self.action_mode && self.action_mode_key(code) {
                            return;
                        }
                        if self
                            .modifiers
                            .contains(ModifiersState::CONTROL | ModifiersState::SHIFT)
                        {
                            match code {
                                KeyCode::KeyF => {
                                    if let Some(overlay) = self.overlay.as_mut() {
                                        overlay.show(Some(Screen::CmdPalette));
                                        self.request_redraw();
                                    }
                                    return;
                                }
                                KeyCode::KeyS => {
                                    if let Some(overlay) = self.overlay.as_mut() {
                                        overlay.show(Some(Screen::Sessions));
                                        self.request_redraw();
                                    }
                                    return;
                                }
                                _ => {}
                            }
                        }
                    } else {
                        if code == KeyCode::KeyB && event.state == ElementState::Released {
                            self.resize_mode_held = false;
                            if self.resize_mode_used {
                                self.action_mode = false;
                            }
                            self.resize_mode_used = false;
                            return;
                        }
                        if event.state != ElementState::Pressed {
                            return;
                        }
                        if self.resize_mode_held {
                            match code {
                                KeyCode::ArrowLeft => {
                                    self.resize_mode_used = true;
                                    self.resize_active_pane(SplitDirection::Vertical, -1);
                                    return;
                                }
                                KeyCode::ArrowRight => {
                                    self.resize_mode_used = true;
                                    self.resize_active_pane(SplitDirection::Vertical, 1);
                                    return;
                                }
                                KeyCode::ArrowUp => {
                                    self.resize_mode_used = true;
                                    self.resize_active_pane(SplitDirection::Horizontal, -1);
                                    return;
                                }
                                KeyCode::ArrowDown => {
                                    self.resize_mode_used = true;
                                    self.resize_active_pane(SplitDirection::Horizontal, 1);
                                    return;
                                }
                                _ => {}
                            }
                        }
                        if self.action_mode && self.action_mode_key(code) {
                            return;
                        }
                        let pressed = KeyBinding {
                            mods: self.modifiers,
                            key: code,
                        };
                        if self.action == pressed {
                            self.resize_mode_held = true;
                            self.resize_mode_used = false;
                            self.action_mode = true;
                            return;
                        } else {
                            for (bind, func) in self.key_bindings.clone() {
                                if bind == pressed {
                                    config::with_env(self, |this| {
                                        func.call::<()>(this)?;
                                        Ok(())
                                    })
                                    .into_log();
                                    self.request_redraw();
                                    return;
                                }
                            }
                        }
                    }
                }

                if let Some(mut overlay) = self.overlay.take() {
                    if overlay.shown() {
                        overlay.handle_input(self, &event);
                        self.request_redraw();
                        self.overlay = Some(overlay);
                        return;
                    }
                    self.overlay = Some(overlay);
                }

                let Some(active_session) = self.active_session() else {
                    return;
                };
                let is_csi = self.is_csi();
                if let Some(session) = self.session_manager.session_mut(active_session) {
                    let reset_scrollback = session.reset_scrollback();
                    session.handle_key_press(&event, self.modifiers, is_csi);
                    if reset_scrollback {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Ime(event) => self.handle_ime_event(event),
            _ => {}
        }
    }
}

impl App {
    fn request_redraw(&self) -> bool {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
            return true;
        }
        false
    }

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

    fn cursor_to_cell(&self, x: f64, y: f64) -> Option<(u16, u16)> {
        if self.cols == 0 || self.rows == 0 || self.font_size <= 0.0 || self.line_height <= 0.0 {
            return None;
        }

        let cell_width = self.font_size / 2.0;
        let col = ((x.max(0.0) as f32) / cell_width).floor() as i32 + 1;
        let row = ((y.max(0.0) as f32) / self.line_height).floor() as i32 + 1;
        let col = col.clamp(1, i32::from(self.cols)) as u16;
        let row = row.clamp(1, i32::from(self.rows)) as u16;
        Some((col, row))
    }

    fn is_csi(&self) -> Option<u8> {
        let mut value = 1u8;
        if self.modifiers.shift_key() {
            value += 1;
        }
        if self.modifiers.alt_key() {
            value += 2;
        }
        if self.modifiers.control_key() {
            value += 4;
        }
        if value == 1 {
            None
        } else {
            Some(value)
        }
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
        if index == self.current_tab {
            self.update_ime_cursor_area();
        }
    }

    fn pane_hit_test(&self, x: f64, y: f64) -> Option<PaneHit> {
        let (col, row) = self.cursor_to_cell(x, y)?;
        for (session_id, geometry) in self.tab_layouts() {
            if !geometry.contains_global_cell(col, row) {
                continue;
            }
            let (col, row) = geometry.local_cell(col, row);
            return Some(PaneHit {
                session_id,
                col,
                row,
            });
        }
        None
    }

    fn divider_hit_test(&self, x: f64, y: f64) -> Option<Divider> {
        const HIT_SLOP: f64 = 6.0;
        let cell_width = f64::from(self.font_size) / 2.0;
        let line_height = f64::from(self.line_height);
        if cell_width <= 0.0 || line_height <= 0.0 {
            return None;
        }
        for divider in self.tab_dividers() {
            match divider.direction {
                SplitDirection::Vertical => {
                    let line_x = cell_width * f64::from(divider.x);
                    let min_y = line_height * f64::from(divider.y);
                    let max_y = line_height * f64::from(divider.y + divider.rows);
                    if (x - line_x).abs() <= HIT_SLOP
                        && y >= min_y - HIT_SLOP
                        && y <= max_y + HIT_SLOP
                    {
                        return Some(divider);
                    }
                }
                SplitDirection::Horizontal => {
                    let line_y = line_height * f64::from(divider.y);
                    let min_x = cell_width * f64::from(divider.x);
                    let max_x = cell_width * f64::from(divider.x + divider.cols);
                    if (y - line_y).abs() <= HIT_SLOP
                        && x >= min_x - HIT_SLOP
                        && x <= max_x + HIT_SLOP
                    {
                        return Some(divider);
                    }
                }
            }
        }
        None
    }

    fn cursor_to_grid_position(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        if self.font_size <= 0.0 || self.line_height <= 0.0 {
            return None;
        }
        let cell_width = self.font_size / 2.0;
        let col = (x.max(0.0) as f32 / cell_width).clamp(0.0, f32::from(self.cols));
        let row =
            (y.max(0.0) as f32 / self.line_height).clamp(0.0, f32::from(self.terminal_rows()));
        Some((col, row))
    }

    fn set_active_session(&mut self, session_id: SessionId) {
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if tab.set_active_session(session_id) {
            self.wheel_remainder = 0.0;
            self.update_ime_cursor_area();
            self.request_redraw();
        }
    }

    fn focus_next_pane(&mut self) {
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if tab.focus_next().is_none() {
            return;
        }
        self.wheel_remainder = 0.0;
        self.update_ime_cursor_area();
        self.request_redraw();
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
        self.request_redraw();
    }

    fn resize_dragged_divider(&mut self, drag: &DividerDrag, x: f64, y: f64) {
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
        self.request_redraw();
    }

    fn split_current_tab(&mut self, direction: SplitDirection) {
        let Some(active_session) = self.active_session() else {
            return;
        };
        let Some((_, geometry)) = self
            .tab_layouts()
            .into_iter()
            .find(|(session_id, _)| *session_id == active_session)
        else {
            return;
        };

        let can_split = match direction {
            SplitDirection::Horizontal => geometry.rows >= 2,
            SplitDirection::Vertical => geometry.cols >= 2,
        };
        if !can_split {
            return;
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
                error!(error = ?error, "failed to split session");
                return;
            }
        };
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if !tab.split_active(direction, session_id) {
            return;
        }

        self.wheel_remainder = 0.0;
        self.resize_tab();
        self.request_redraw();
    }

    fn switch_tab(&mut self, tab: usize) {
        if self.tabs[tab].is_none() {
            let id = match self.session_manager.create_session(
                self.terminal_rows().max(1),
                self.cols.max(1),
                self.default_cwd.as_deref(),
            ) {
                Ok(id) => id,
                Err(error) => {
                    error!(error = ?error, "failed to create tab session");
                    return;
                }
            };
            self.tabs[tab] = Some(Tab::new(id));
        }

        self.current_tab = tab;
        self.wheel_remainder = 0.0;
        self.resize_tab();
        self.update_ime_cursor_area();
        self.request_redraw();
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
        self.request_redraw();
        true
    }

    fn detach_active_session(&mut self) {
        let Some(active_session) = self.active_session() else {
            return;
        };
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if !tab.remove_session(active_session) {
            return;
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
        self.update_ime_cursor_area();
        self.request_redraw();
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
        self.update_ime_cursor_area();
        self.request_redraw();
        true
    }

    pub fn live_detached_sessions(&self) -> Vec<SessionId> {
        self.detached_sessions
            .iter()
            .copied()
            .filter(|id| self.session_manager.session(*id).is_some())
            .collect()
    }

    fn rename_active(&mut self, name: &str) {
        let Some(session) = self.active_session() else {
            return;
        };
        let Some(session) = self.session_manager.session_mut(session) else {
            return;
        };
        session.rename(name.to_string());
        self.request_redraw();
    }

    fn open_status_prompt(&mut self) {
        self.status_bar_hidden = false;
        self.status_message = None;
        self.status_prompt = Some(StatusPrompt::new(StatusPromptMode::Command));
        self.resize_tab();
        self.request_redraw();
    }

    fn open_rename_prompt(&mut self) {
        self.status_bar_hidden = false;
        self.status_message = None;
        let mut prompt = StatusPrompt::new(StatusPromptMode::Rename);
        let name = self
            .active_session()
            .and_then(|id| self.session_manager.session(id))
            .map(TerminalSession::title)
            .unwrap_or_default();
        if !name.is_empty() {
            prompt.insert(name);
        }
        self.status_prompt = Some(prompt);
        self.resize_tab();
        self.request_redraw();
    }

    fn open_lua_prompt(&mut self) {
        self.status_bar_hidden = false;
        self.status_message = None;
        self.status_prompt = Some(StatusPrompt::new(StatusPromptMode::Lua));
        self.resize_tab();
        self.request_redraw();
    }

    fn handle_status_prompt(&mut self, mut prompt: StatusPrompt, event: &KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else {
            self.status_prompt = Some(prompt);
            return;
        };
        if event.state != ElementState::Pressed {
            self.status_prompt = Some(prompt);
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
                        StatusPromptMode::Command => {
                            prompt.push_history(input.clone());
                            self.execute_status_command(&input);
                        }
                        StatusPromptMode::Rename => {
                            self.rename_active(&input);
                        }
                        StatusPromptMode::Lua => {
                            prompt.push_history(input.clone());
                            self.evaluate_lua(&input);
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
            self.status_prompt = Some(prompt);
        }
        self.request_redraw();
    }

    fn execute_status_command(&mut self, input: &str) {
        let mut overlay = self.overlay.take();
        let handled = match overlay.as_mut() {
            Some(overlay) => overlay.execute_command(self, input),
            None => false,
        };
        self.overlay = overlay;
        if !handled {
            self.status_message =
                Some(format!("unknown command: {}", input.split(' ').next().unwrap_or(input)));
        }
    }

    fn evaluate_lua(&mut self, expr: &str) {
        let lua = self.lua.clone();
        let result = RefCell::new(None::<String>);
        let out = config::with_env(self, |_| {
            let value = lua.load(expr).eval::<LuaValue>()?;
            *result.borrow_mut() = Some(Self::format_lua_value(&lua, value, 0)?);
            Ok(())
        });
        let message = match out {
            Err(error) => format!("{error}"),
            Ok(()) => result.into_inner().unwrap_or_else(|| "nil".to_string()),
        };
        self.status_message = Some(message.replace(['\n', '\r'], " "));
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

    fn status_input(&self) -> Option<StatusInput> {
        let prompt = self.status_prompt.as_ref()?;
        let left_limit = usize::from(self.cols.max(1));

        let prompt_text = match prompt.mode {
            StatusPromptMode::Command => " : ",
            StatusPromptMode::Rename => " R: ",
            StatusPromptMode::Lua => " > ",
        };
        let prompt_cols = prompt_text.chars().count();

        let text = &prompt.buffer;
        let cursor_col = text[..prompt.cursor.min(text.len())].chars().count();
        let max_cols = left_limit.saturating_sub(prompt_cols).max(1);

        let start = if cursor_col >= max_cols {
            cursor_col - max_cols + 1
        } else {
            0
        };
        let visible = text.chars().skip(start).take(max_cols).collect::<String>();
        let visible_cursor = cursor_col.saturating_sub(start);

        Some(StatusInput {
            prompt: prompt_text.to_string(),
            text: visible,
            cursor_col: prompt_cols + visible_cursor,
        })
    }

    fn action_mode_key(&mut self, code: KeyCode) -> bool {
        if self.pending_move_to_tab {
            self.pending_move_to_tab = false;
            self.action_mode = false;
            self.request_redraw();
            if let Some(target) = Self::tab_digit_index(code)
                && let Some(session) = self.active_session()
            {
                self.move_session_to_tab(session, target);
            }
            return true;
        }

        self.action_mode = false;
        self.request_redraw();
        match code {
            KeyCode::Digit1 => self.switch_tab(0),
            KeyCode::Digit2 => self.switch_tab(1),
            KeyCode::Digit3 => self.switch_tab(2),
            KeyCode::Digit4 => self.switch_tab(3),
            KeyCode::Digit5 => self.switch_tab(4),
            KeyCode::Digit6 => self.switch_tab(5),
            KeyCode::Digit7 => self.switch_tab(6),
            KeyCode::Digit8 => self.switch_tab(7),
            KeyCode::Digit9 => self.switch_tab(8),
            KeyCode::KeyK => self.switch_tab(self.next_tab_index()),
            KeyCode::KeyJ => self.switch_tab(self.previous_tab_index()),
            KeyCode::KeyW => self.focus_next_pane(),
            KeyCode::KeyV => self.split_current_tab(SplitDirection::Vertical),
            KeyCode::KeyH => self.split_current_tab(SplitDirection::Horizontal),
            KeyCode::KeyT => {
                if let Some(window) = self.window.as_mut() {
                    window.set_decorations(!window.is_decorated());
                    window.request_redraw();
                }
            }
            KeyCode::KeyS => {
                self.status_bar_hidden = !self.status_bar_hidden;
                self.resize_tab();
                self.request_redraw();
            }
            KeyCode::KeyP => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.show(Some(Screen::CmdPalette));
                }
                self.request_redraw();
            }
            KeyCode::Semicolon => {
                self.open_status_prompt();
            }
            KeyCode::KeyL => {
                self.open_lua_prompt();
            }
            KeyCode::KeyM => {
                self.pending_move_to_tab = true;
                self.action_mode = true;
                self.request_redraw();
            }
            KeyCode::KeyD => {
                self.detach_active_session();
            }
            KeyCode::KeyR => {
                self.open_rename_prompt();
            }
            KeyCode::KeyA => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.show(Some(Screen::Detached));
                }
                self.request_redraw();
            }
            _ => return false,
        }
        true
    }

    fn tab_digit_index(code: KeyCode) -> Option<usize> {
        match code {
            KeyCode::Digit1 => Some(0),
            KeyCode::Digit2 => Some(1),
            KeyCode::Digit3 => Some(2),
            KeyCode::Digit4 => Some(3),
            KeyCode::Digit5 => Some(4),
            KeyCode::Digit6 => Some(5),
            KeyCode::Digit7 => Some(6),
            KeyCode::Digit8 => Some(7),
            KeyCode::Digit9 => Some(8),
            _ => None,
        }
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

    fn status_tabs(&self) -> Option<Vec<StatusTab>> {
        if self.status_bar_hidden {
            return None;
        }

        Some(
            self.tabs
                .iter()
                .enumerate()
                .filter_map(|(index, tab)| {
                    tab.as_ref().map(|_| StatusTab {
                        label: format!("{} {}", index + 1, self.tab_program_name(index)),
                        is_active: index == self.current_tab,
                    })
                })
                .collect(),
        )
    }

    fn handle_ime_event(&mut self, event: Ime) {
        match event {
            Ime::Enabled => {
                self.ime_enabled = true;
                self.update_ime_cursor_area();
            }
            Ime::Preedit(text, _) => {
                self.ime_preedit = (!text.is_empty()).then_some(text);
                self.update_ime_cursor_area();
                self.request_redraw();
            }
            Ime::Commit(text) => {
                self.ime_preedit = None;
                let Some(active_session) = self.active_session() else {
                    return;
                };
                let reset_scrollback = self
                    .session_manager
                    .session_mut(active_session)
                    .is_some_and(TerminalSession::reset_scrollback);
                self.session_manager.send_text(active_session, &text);
                if reset_scrollback {
                    self.request_redraw();
                }
                self.update_ime_cursor_area();
            }
            Ime::Disabled => {
                self.ime_enabled = false;
                self.ime_preedit = None;
                self.request_redraw();
            }
        }
    }

    fn update_ime_cursor_area(&self) {
        if !self.ime_enabled {
            return;
        }
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Some(active_session) = self.active_session() else {
            return;
        };
        let Some(session) = self.session_manager.session(active_session) else {
            return;
        };
        let Some((_, geometry)) = self
            .tab_layouts()
            .into_iter()
            .find(|(session_id, _)| *session_id == active_session)
        else {
            return;
        };

        let (row, col) = session.vt.screen().cursor_position();
        let cell_width = self.font_size / 2.0;
        let x = cell_width * (f32::from(geometry.x) + f32::from(col));
        let y = self.line_height * (f32::from(geometry.y) + f32::from(row));

        window.set_ime_cursor_area(
            PhysicalPosition::new(x as i32, y as i32),
            PhysicalSize::new(cell_width.ceil() as u32, self.line_height.ceil() as u32),
        );
    }

    fn ime_preedit(&self) -> Option<ImePreedit> {
        let text = self.ime_preedit.as_ref()?;
        let active_session = self.active_session()?;
        let session = self.session_manager.session(active_session)?;
        let (_, geometry) = self
            .tab_layouts()
            .into_iter()
            .find(|(session_id, _)| *session_id == active_session)?;
        let (row, col) = session.vt.screen().cursor_position();

        Some(ImePreedit {
            text: text.clone(),
            geometry,
            row,
            col,
        })
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
                    error!(error = ?error, "failed to create session in dir");
                    return;
                }
            };
            self.tabs[free] = Some(Tab::new(id));
            self.current_tab = free;
            self.wheel_remainder = 0.0;
            self.resize_tab();
            self.update_ime_cursor_area();
            self.request_redraw();
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
                    error!(error = ?error, "failed to create session in dir");
                    return;
                }
            };
            let Some(tab) = &mut self.tabs[self.current_tab] else {
                return;
            };
            tab.split_active(SplitDirection::Horizontal, id);
            self.request_redraw();
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
                    error!(error = ?error, "failed to create tab session");
                    return;
                }
            };
            self.tabs[free] = Some(Tab::new(id));
            self.current_tab = free;
            self.wheel_remainder = 0.0;
            self.resize_tab();
            self.update_ime_cursor_area();
            self.request_redraw();
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
                    error!(error = ?error, "failed to create tab session");
                    return;
                }
            };

            let Some(tab) = &mut self.tabs[self.current_tab] else {
                return;
            };

            tab.split_active(SplitDirection::Horizontal, id);
            self.request_redraw();
        }
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
