#![cfg_attr(all(windows, feature = "install"), windows_subsystem = "windows")]

mod pty;
mod renderer;
mod terminal;
//mod ui;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::Parser;
use pty::Event as PtyEvent;
use renderer::Renderer;
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{Icon, Window, WindowId},
};

use crate::terminal::{SessionId, SessionManager};

#[derive(clap::Parser)]
struct Cli {
    path: Option<PathBuf>,
}

struct App {
    args: Cli,
    renderer: Option<Renderer>,
    window: Option<Arc<Window>>,
    session_manager: SessionManager,
    modifiers: ModifiersState,
    line_height: f32,
    font_size: f32,
    rows: u16,
    cols: u16,
    tabs: [Option<SessionId>; 9],
    action_mode: bool,
    current_tab: usize,
    cursor_pos: Option<(f64, f64)>,
    wheel_remainder: f32,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let event_loop = EventLoop::<PtyEvent>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    let proxy = event_loop.create_proxy();
    let mut app = App {
        args: cli,
        renderer: None,
        window: None,
        session_manager: SessionManager::new(proxy),
        modifiers: ModifiersState::default(),
        font_size: 24.0,
        line_height: 28.0,
        rows: 20,
        cols: 80,
        tabs: [None; 9],
        action_mode: false,
        current_tab: 0,
        cursor_pos: None,
        wheel_remainder: 0.0,
    };

    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app)?;

    Ok(())
}

impl App {
    const TITLE: &str = "Pyonji";
    const ICON: &[u8] = include_bytes!("../resources/icon.ico");
}

impl ApplicationHandler<PtyEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let icon = image::load_from_memory(Self::ICON)
            .ok()
            .map(|image| {
                let data = image.to_rgba8().to_vec();
                Icon::from_rgba(data, image.width(), image.height())
                    .inspect_err(|e| println!("icon-err = {e}"))
                    .ok()
            })
            .flatten();
        let Ok(window) = event_loop.create_window(
            Window::default_attributes()
                .with_inner_size(PhysicalSize::new(1280, 720))
                .with_active(true)
                .with_window_icon(icon)
                .with_title(Self::TITLE),
        ) else {
            event_loop.exit();
            return;
        };
        window.set_ime_allowed(true);
        let size = window.inner_size();
        let rows = (size.height as f32 / self.line_height) as u16;
        let cols = (size.width as f32 / (self.font_size / 2.0)) as u16;
        let window = Arc::new(window);
        self.renderer = Renderer::new(window.clone(), self.font_size, self.line_height).ok();
        self.window = Some(window.clone());
        self.rows = rows;
        self.cols = cols;
        if let Ok(session) = self.session_manager.create_session(
            rows,
            cols,
            self.args.path.as_ref().map(|pb| pb.as_path()),
        ) {
            self.session_manager.set_active_session(session);
            self.tabs[0] = Some(session);
        }
        window.request_redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: PtyEvent) {
        match event {
            PtyEvent::Closed(id) => {
                self.session_manager.remove_session(id);
                if self.session_manager.is_empty() {
                    event_loop.exit();
                }
                if let Some(tab) = self.tabs.iter().position(|session| *session == Some(id)) {
                    self.tabs[tab] = None;
                    if self.current_tab == tab {
                        self.switch_to_previous_live_tab_or_stay(tab);
                    }
                }
                self.window.as_ref().map(|window| window.request_redraw());
            }
            PtyEvent::Data(id, data) => {
                self.session_manager.update_session(id, &data);
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
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
                if let Some(renderer) = self.renderer.as_mut() {
                    if let Some(session) = self.session_manager.active_session() {
                        if let Err(e) = renderer.render(session.vt.screen(), &session.cursor_style)
                        {
                            println!("failed to render = {e}");
                        }
                    }
                }
            }
            WindowEvent::Resized(size) => {
                if size.width == 0 || size.height == 0 {
                    return;
                }
                let rows = (size.height as f32 / self.line_height) as u16;
                let cols = (size.width as f32 / (self.font_size / 2.0)) as u16;
                self.session_manager.resize_sessions(rows, cols);
                self.rows = rows;
                self.cols = cols;
                let Some((window, renderer)) = self.window.as_ref().zip(self.renderer.as_mut())
                else {
                    return;
                };
                renderer.resize(size);
                window.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = Some((position.x, position.y));
                if let Some((col, row)) = self.cursor_to_cell(position.x, position.y) {
                    if let Some(session) = self.session_manager.active_session_mut() {
                        let reset_scrollback = if session.uses_local_scrollback() {
                            false
                        } else {
                            session.reset_scrollback()
                        };
                        session.handle_mouse_move(self.modifiers, col, row);
                        if reset_scrollback {
                            if let Some(window) = self.window.as_ref() {
                                window.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some((x, y)) = self.cursor_pos else {
                    return;
                };
                let Some((col, row)) = self.cursor_to_cell(x, y) else {
                    return;
                };
                if let Some(session) = self.session_manager.active_session_mut() {
                    let reset_scrollback = if session.uses_local_scrollback() {
                        false
                    } else {
                        session.reset_scrollback()
                    };
                    session.handle_mouse_button(button, state, self.modifiers, col, row);
                    if reset_scrollback {
                        if let Some(window) = self.window.as_ref() {
                            window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some((x, y)) = self.cursor_pos else {
                    return;
                };
                let Some((col, row)) = self.cursor_to_cell(x, y) else {
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
                    .active_session()
                    .is_some_and(|session| session.uses_local_scrollback());
                let whole_lines = if uses_local_scrollback {
                    self.take_wheel_steps(lines)
                } else {
                    self.wheel_remainder = 0.0;
                    0
                };
                if let Some(session) = self.session_manager.active_session_mut() {
                    if uses_local_scrollback {
                        if whole_lines != 0 && session.scroll_scrollback(whole_lines) {
                            if let Some(window) = self.window.as_ref() {
                                window.request_redraw();
                            }
                        }
                    } else {
                        let reset_scrollback = session.reset_scrollback();
                        session.handle_mouse_wheel(lines, self.modifiers, col, row);
                        if reset_scrollback {
                            if let Some(window) = self.window.as_ref() {
                                window.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    if self.modifiers.control_key() && matches!(code, KeyCode::KeyB) {
                        self.action_mode = true;
                        return;
                    } else if self.action_mode {
                        self.action_mode = false;
                        match code {
                            KeyCode::Digit1 => {
                                self.switch_tab(0);
                                return;
                            }
                            KeyCode::Digit2 => {
                                self.switch_tab(1);
                                return;
                            }
                            KeyCode::Digit3 => {
                                self.switch_tab(2);
                                return;
                            }
                            KeyCode::Digit4 => {
                                self.switch_tab(3);
                                return;
                            }
                            KeyCode::Digit5 => {
                                self.switch_tab(4);
                                return;
                            }
                            KeyCode::Digit6 => {
                                self.switch_tab(5);
                                return;
                            }
                            KeyCode::Digit7 => {
                                self.switch_tab(6);
                                return;
                            }
                            KeyCode::Digit8 => {
                                self.switch_tab(7);
                                return;
                            }
                            KeyCode::Digit9 => {
                                self.switch_tab(8);
                                return;
                            }
                            KeyCode::KeyN => {
                                self.switch_tab(self.next_tab_index());
                                return;
                            }
                            KeyCode::KeyP => {
                                self.switch_tab(self.previous_tab_index());
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                let is_csi = self.is_csi();
                if let Some(session) = self.session_manager.active_session_mut() {
                    let reset_scrollback = session.reset_scrollback();
                    session.handle_key_press(&event, self.modifiers, is_csi);
                    if reset_scrollback {
                        if let Some(window) = self.window.as_ref() {
                            window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::CloseRequested => {
                event_loop.exit();
                return;
            }
            WindowEvent::Ime(ime) => {
                println!("ime = {ime:?}");
                //Ime::Preedit
            }
            _ => {}
        }
    }
}

impl App {
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
            self.switch_tab(tab);
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
        let col = col.clamp(1, self.cols as i32) as u16;
        let row = row.clamp(1, self.rows as i32) as u16;
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

    fn switch_tab(&mut self, tab: usize) {
        if self.session_manager.active_session_id() == self.tabs[tab] {
            return;
        }
        if let Some(id) = self.tabs[tab] {
            self.session_manager.set_active_session(id);
            self.current_tab = tab;
            self.wheel_remainder = 0.0;
            if let Some(window) = self.window.as_ref() {
                window.set_title(&format!("{} - Tab: {}", Self::TITLE, self.current_tab));
                window.request_redraw();
            }
        } else {
            if let Ok(id) = self
                .session_manager
                .create_session(self.rows, self.cols, None)
            {
                self.tabs[tab] = Some(id);
                self.session_manager.set_active_session(id);
                self.current_tab = tab;
                self.wheel_remainder = 0.0;
                if let Some(window) = self.window.as_ref() {
                    window.set_title(&format!("{} - Tab: {}", Self::TITLE, self.current_tab));
                    window.request_redraw();
                }
            }
        }
    }
}
