#![cfg_attr(all(windows, feature = "install"), windows_subsystem = "windows")]

mod pty;
mod renderer;
mod terminal;
//mod ui;

use std::{array, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::Parser;
use pty::Event as PtyEvent;
use renderer::{RenderPane, Renderer};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{Icon, Window, WindowId},
};

use crate::terminal::{
    Divider, PaneGeometry, PanePathStep, SessionId, SessionManager, SplitDirection, Tab,
};

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
    tabs: [Option<Tab>; 9],
    action_mode: bool,
    current_tab: usize,
    cursor_pos: Option<(f64, f64)>,
    wheel_remainder: f32,
    divider_drag: Option<DividerDrag>,
    resize_mode_held: bool,
    resize_mode_used: bool,
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
        tabs: array::from_fn(|_| None),
        action_mode: false,
        current_tab: 0,
        cursor_pos: None,
        wheel_remainder: 0.0,
        divider_drag: None,
        resize_mode_held: false,
        resize_mode_used: false,
    };

    event_loop.set_control_flow(ControlFlow::Wait);
    if let Err(e) = event_loop.run_app(&mut app) {
        std::fs::write("C:/dev/learning/pyonji/log.txt", format!("error = {e}"))?;
        return Err(e.into());
    }
    Ok(())
}

impl App {
    const TITLE: &str = "Pyonji";
    const ICON: &[u8] = include_bytes!("../resources/icon.ico");
}

impl ApplicationHandler<PtyEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let icon = image::load_from_memory(Self::ICON).ok().and_then(|image| {
            let data = image.to_rgba8().to_vec();
            Icon::from_rgba(data, image.width(), image.height())
                .inspect_err(|e| println!("icon-err = {e}"))
                .ok()
        });
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
        self.rows = (size.height as f32 / self.line_height) as u16;
        self.cols = (size.width as f32 / (self.font_size / 2.0)) as u16;

        let window = Arc::new(window);
        self.renderer = Renderer::new(window.clone(), self.font_size, self.line_height).ok();
        self.window = Some(window.clone());

        if let Ok(session) = self.session_manager.create_session(
            self.rows.max(1),
            self.cols.max(1),
            self.args.path.as_deref(),
        ) {
            self.tabs[0] = Some(Tab::new(session));
            self.current_tab = 0;
            self.resize_current_tab_sessions();
            self.update_window_title();
        }
        window.request_redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: PtyEvent) {
        match event {
            PtyEvent::Closed(id) => {
                self.session_manager.remove_session(id);
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
                    self.resize_current_tab_sessions();
                    self.update_window_title();
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
                let panes = self.current_tab_layouts();
                let dividers = self.current_tab_dividers();
                let active_session = self.current_active_session_id();
                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };

                let mut render_panes = Vec::with_capacity(panes.len());
                for (session_id, geometry) in panes {
                    let Some(session) = self.session_manager.session(session_id) else {
                        continue;
                    };
                    render_panes.push(RenderPane {
                        screen: session.vt.screen(),
                        cursor_style: &session.cursor_style,
                        geometry,
                        is_active: Some(session_id) == active_session,
                    });
                }

                if let Err(e) = renderer.render(&render_panes, &dividers) {
                    println!("failed to render = {e}");
                }
            }
            WindowEvent::Resized(size) => {
                if size.width == 0 || size.height == 0 {
                    return;
                }
                self.rows = (size.height as f32 / self.line_height) as u16;
                self.cols = (size.width as f32 / (self.font_size / 2.0)) as u16;
                self.resize_current_tab_sessions();

                let Some((window, renderer)) = self.window.as_ref().zip(self.renderer.as_mut())
                else {
                    return;
                };
                renderer.resize(size);
                window.request_redraw();
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
                        if let Some(window) = self.window.as_ref() {
                            window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some((x, y)) = self.cursor_pos else {
                    return;
                };

                if button == winit::event::MouseButton::Left {
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
                        _ => {}
                    }
                }

                let Some(hit) = self.pane_hit_test(x, y) else {
                    return;
                };

                if state == ElementState::Pressed {
                    self.set_active_session(hit.session_id);
                }

                if let Some(session) = self.session_manager.session_mut(hit.session_id) {
                    let reset_scrollback = if session.uses_local_scrollback() {
                        false
                    } else {
                        session.reset_scrollback()
                    };
                    session.handle_mouse_button(button, state, self.modifiers, hit.col, hit.row);
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
                    .is_some_and(|session| session.uses_local_scrollback());
                let whole_lines = if uses_local_scrollback {
                    self.take_wheel_steps(lines)
                } else {
                    self.wheel_remainder = 0.0;
                    0
                };

                if let Some(session) = self.session_manager.session_mut(hit.session_id) {
                    if uses_local_scrollback {
                        if whole_lines != 0 && session.scroll_scrollback(whole_lines) {
                            if let Some(window) = self.window.as_ref() {
                                window.request_redraw();
                            }
                        }
                    } else {
                        let reset_scrollback = session.reset_scrollback();
                        session.handle_mouse_wheel(lines, self.modifiers, hit.col, hit.row);
                        if reset_scrollback {
                            if let Some(window) = self.window.as_ref() {
                                window.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
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
                            KeyCode::KeyW => {
                                self.focus_next_pane();
                                return;
                            }
                            KeyCode::KeyV => {
                                self.split_current_tab(SplitDirection::Vertical);
                                return;
                            }
                            KeyCode::KeyH => {
                                self.split_current_tab(SplitDirection::Horizontal);
                                return;
                            }
                            KeyCode::ArrowLeft => {
                                self.resize_active_pane(SplitDirection::Vertical, -1);
                                return;
                            }
                            KeyCode::ArrowRight => {
                                self.resize_active_pane(SplitDirection::Vertical, 1);
                                return;
                            }
                            KeyCode::ArrowUp => {
                                self.resize_active_pane(SplitDirection::Horizontal, -1);
                                return;
                            }
                            KeyCode::ArrowDown => {
                                self.resize_active_pane(SplitDirection::Horizontal, 1);
                                return;
                            }
                            _ => {}
                        }
                    }
                }

                let Some(active_session) = self.current_active_session_id() else {
                    return;
                };
                let is_csi = self.is_csi();
                if let Some(session) = self.session_manager.session_mut(active_session) {
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
            }
            WindowEvent::Ime(ime) => {
                println!("ime = {ime:?}");
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
            self.current_tab = tab;
            self.wheel_remainder = 0.0;
            self.resize_current_tab_sessions();
            self.update_window_title();
            return;
        }

        self.current_tab = closed_tab.min(self.tabs.len().saturating_sub(1));
        self.update_window_title();
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

    fn current_active_session_id(&self) -> Option<SessionId> {
        self.tabs[self.current_tab]
            .as_ref()
            .and_then(Tab::active_session)
    }

    fn current_tab_layouts(&self) -> Vec<(SessionId, PaneGeometry)> {
        self.tabs[self.current_tab]
            .as_ref()
            .map(|tab| {
                tab.layout(PaneGeometry {
                    x: 0,
                    y: 0,
                    cols: self.cols,
                    rows: self.rows,
                })
            })
            .unwrap_or_default()
    }

    fn current_tab_dividers(&self) -> Vec<Divider> {
        self.tabs[self.current_tab]
            .as_ref()
            .map(|tab| {
                tab.dividers(PaneGeometry {
                    x: 0,
                    y: 0,
                    cols: self.cols,
                    rows: self.rows,
                })
            })
            .unwrap_or_default()
    }

    fn resize_current_tab_sessions(&mut self) {
        for (session_id, geometry) in self.current_tab_layouts() {
            self.session_manager.resize_session(
                session_id,
                geometry.rows.max(1),
                geometry.cols.max(1),
            );
        }
    }

    fn pane_hit_test(&self, x: f64, y: f64) -> Option<PaneHit> {
        let (col, row) = self.cursor_to_cell(x, y)?;
        for (session_id, geometry) in self.current_tab_layouts() {
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
        let cell_width = self.font_size as f64 / 2.0;
        let line_height = self.line_height as f64;
        if cell_width <= 0.0 || line_height <= 0.0 {
            return None;
        }
        const HIT_SLOP: f64 = 6.0;
        for divider in self.current_tab_dividers() {
            match divider.direction {
                SplitDirection::Vertical => {
                    let line_x = cell_width * divider.x as f64;
                    let min_y = line_height * divider.y as f64;
                    let max_y = line_height * (divider.y + divider.rows) as f64;
                    if (x - line_x).abs() <= HIT_SLOP
                        && y >= min_y - HIT_SLOP
                        && y <= max_y + HIT_SLOP
                    {
                        return Some(divider);
                    }
                }
                SplitDirection::Horizontal => {
                    let line_y = line_height * divider.y as f64;
                    let min_x = cell_width * divider.x as f64;
                    let max_x = cell_width * (divider.x + divider.cols) as f64;
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
        let col = (x.max(0.0) as f32 / cell_width).clamp(0.0, self.cols as f32);
        let row = (y.max(0.0) as f32 / self.line_height).clamp(0.0, self.rows as f32);
        Some((col, row))
    }

    fn set_active_session(&mut self, session_id: SessionId) {
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if tab.set_active_session(session_id) {
            self.wheel_remainder = 0.0;
            self.update_window_title();
            if let Some(window) = self.window.as_ref() {
                window.request_redraw();
            }
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
        self.update_window_title();
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    fn resize_active_pane(&mut self, direction: SplitDirection, delta_first: i16) {
        let area = PaneGeometry {
            x: 0,
            y: 0,
            cols: self.cols,
            rows: self.rows,
        };
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if !tab.resize_active_split(area, direction, delta_first) {
            return;
        }
        self.wheel_remainder = 0.0;
        self.resize_current_tab_sessions();
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
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
            rows: self.rows,
        };
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if !tab.resize_split_by_position(area, &drag.path, drag.direction, position) {
            return;
        }
        self.wheel_remainder = 0.0;
        self.resize_current_tab_sessions();
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    fn split_current_tab(&mut self, direction: SplitDirection) {
        let Some(active_session) = self.current_active_session_id() else {
            return;
        };
        let Some((_, geometry)) = self
            .current_tab_layouts()
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

        let Ok(session_id) = self.session_manager.create_session(
            new_rows,
            new_cols,
            self.session_manager
                .session(active_session)
                .and_then(|session| session.pty.current_dir())
                .as_deref(),
        ) else {
            return;
        };
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if !tab.split_active(direction, session_id) {
            return;
        }

        self.wheel_remainder = 0.0;
        self.resize_current_tab_sessions();
        self.update_window_title();
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    fn switch_tab(&mut self, tab: usize) {
        if self.tabs[tab].is_none() {
            let Ok(id) =
                self.session_manager
                    .create_session(self.rows.max(1), self.cols.max(1), None)
            else {
                return;
            };
            self.tabs[tab] = Some(Tab::new(id));
        }

        self.current_tab = tab;
        self.wheel_remainder = 0.0;
        self.resize_current_tab_sessions();
        self.update_window_title();
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    fn update_window_title(&self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let pane_count = self.tabs[self.current_tab]
            .as_ref()
            .map(|tab| tab.sessions().len())
            .unwrap_or(0);
        if pane_count > 1 {
            window.set_title(&format!(
                "{} - Tab: {} - Panes: {}",
                Self::TITLE,
                self.current_tab + 1,
                pane_count
            ));
        } else {
            window.set_title(&format!("{} - Tab: {}", Self::TITLE, self.current_tab + 1));
        }
    }
}
