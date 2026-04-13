#![cfg_attr(all(windows, feature = "install"), windows_subsystem = "windows")]

mod puty;
mod renderer;
mod terminal;
//mod ui;

use std::sync::Arc;

use anyhow::{Context, Result};
use puty::Event as PtyEvent;
use renderer::Renderer;
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    platform::windows::IconExtWindows,
    window::{Icon, Window, WindowId},
};

use crate::terminal::{SessionId, SessionManager};

struct App {
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
}

fn main() -> Result<()> {
    let event_loop = EventLoop::<PtyEvent>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    let proxy = event_loop.create_proxy();
    let mut app = App {
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
    };

    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app)?;

    Ok(())
}

impl App {
    const TITLE: &str = "Pyonji";
}

impl ApplicationHandler<PtyEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let icon = Icon::from_path("./resources/icon.ico", None)
            .inspect_err(|e| println!("icon-err = {e}"))
            .ok();
        let Ok(window) = event_loop.create_window(
            Window::default_attributes()
                .with_inner_size(PhysicalSize::new(1280, 720))
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
        if let Ok(session) = self.session_manager.create_session(rows, cols) {
            self.session_manager.set_active_session(session);
            self.tabs[0] = Some(session);
        }
        window.request_redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: PtyEvent) {
        match event {
            PtyEvent::Closed(id) => {
                println!("session closed = {id:?}");
                self.session_manager.remove_session(id);
                if self.session_manager.is_empty() {
                    event_loop.exit();
                }
                self.tabs[self.current_tab] = None;
                self.switch_tab(self.current_tab.saturating_sub(1));
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
                        session.handle_mouse_move(self.modifiers, col, row);
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
                    session.handle_mouse_button(button, state, self.modifiers, col, row);
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
                if let Some(session) = self.session_manager.active_session_mut() {
                    session.handle_mouse_wheel(lines, self.modifiers, col, row);
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
                            KeyCode::KeyN => {
                                self.switch_tab((self.current_tab + 1).max(5));
                                return;
                            }
                            KeyCode::KeyP => {
                                self.switch_tab((self.current_tab - 1).min(0));
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                let is_csi = self.is_csi();
                if let Some(session) = self.session_manager.active_session_mut() {
                    session.handle_key_press(&event, self.modifiers, is_csi);
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

    fn switch_tab(&mut self, tab: usize) {
        if self.session_manager.active_session_id() == self.tabs[tab] {
            return;
        }
        if let Some(id) = self.tabs[tab] {
            self.session_manager.set_active_session(id);
            self.current_tab = tab;
            if let Some(window) = self.window.as_ref() {
                window.set_title(&format!("{} - Tab: {}", Self::TITLE, self.current_tab));
                window.request_redraw();
            }
        } else {
            if let Ok(id) = self.session_manager.create_session(self.rows, self.cols) {
                self.tabs[tab] = Some(id);
                self.session_manager.set_active_session(id);
                self.current_tab = tab;
                if let Some(window) = self.window.as_ref() {
                    window.set_title(&format!("{} - Tab: {}", Self::TITLE, self.current_tab));
                    window.request_redraw();
                }
            }
        }
    }
}
