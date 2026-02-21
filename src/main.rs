mod puty;
mod renderer;
mod terminal;

use std::sync::Arc;

use anyhow::{Context, Result};
use puty::{Event as PtyEvent, Pty};
use renderer::Renderer;
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey},
    window::{Window, WindowId},
};

#[derive(Debug)]
enum CursorState {
    Bar,
    Block,
    Underline,
}

struct App {
    pty: Pty,
    vt: vt100::Parser,
    renderer: Option<Renderer>,
    window: Option<Arc<Window>>,
    modifiers: ModifiersState,
    line_height: f32,
    font_size: f32,
    rows: u16,
    cols: u16,
    cursor_style: CursorState,
    scrollback: usize,
}

fn main() -> Result<()> {
    let event_loop = EventLoop::<PtyEvent>::with_user_event()
        .build()
        .context("failed to create event loop")?;
    let proxy = event_loop.create_proxy();

    let mut app = App {
        pty: Pty::new(20, 80, proxy)?,
        vt: vt100::Parser::new(20, 80, 2000),
        renderer: None,
        window: None,
        modifiers: ModifiersState::default(),
        font_size: 24.0,
        line_height: 28.0,
        rows: 20,
        cols: 80,
        cursor_style: CursorState::Bar,
        scrollback: 0,
    };

    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app)?;

    Ok(())
}

impl ApplicationHandler<PtyEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Ok(window) = event_loop.create_window(
            Window::default_attributes()
                .with_inner_size(PhysicalSize::new(1280, 720))
                .with_title("Pty"),
        ) else {
            event_loop.exit();
            return;
        };
        let size = window.inner_size();
        let rows = (size.height as f32 / self.line_height) as u16;
        let cols = (size.width as f32 / (self.font_size / 2.0)) as u16;
        let window = Arc::new(window);
        self.renderer = Renderer::new(window.clone(), self.font_size, self.line_height).ok();
        self.window = Some(window.clone());
        self.pty.resize(rows, cols);
        self.vt.set_size(rows, cols);
        self.rows = rows;
        self.cols = cols;
        window.request_redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: PtyEvent) {
        match event {
            PtyEvent::Closed => {
                event_loop.exit();
            }
            PtyEvent::Data(data) => {
                self.interrupt_pty_data(&data);
                self.vt.process(&data);
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
                    if let Err(e) = renderer.render(self.vt.screen(), &self.cursor_style) {
                        println!("failed to render = {e}");
                    }
                }
            }
            WindowEvent::Resized(size) => {
                if size.width == 0 || size.height == 0 {
                    return;
                }
                let rows = (size.height as f32 / self.line_height) as u16;
                let cols = (size.width as f32 / (self.font_size / 2.0)) as u16;
                self.pty.resize(rows, cols);
                self.vt.set_size(rows, cols);
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
            WindowEvent::KeyboardInput { event, .. } => {
                handle_key_press(self, &event);
            }
            WindowEvent::CloseRequested => {
                event_loop.exit();
                return;
            }
            _ => {}
        }
    }
}

impl App {
    fn interrupt_pty_data(&mut self, data: &[u8]) {
        let Ok(str) = std::str::from_utf8(data) else {
            return;
        };
        for m in cansi::parse(str) {
            match m.text {
                "\x1b[1 q" | "\x1b[2 q" => {
                    self.cursor_style = CursorState::Block;
                }
                "\x1b[3 q" | "\x1b[4 q" => {
                    self.cursor_style = CursorState::Underline;
                }
                "\x1b[0 q" | "\x1b[5 q" | "\x1b[6 q" => {
                    self.cursor_style = CursorState::Bar;
                }
                _ => {}
            }
        }
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
}

fn handle_key_press(app: &mut App, event: &KeyEvent) {
    if event.state != ElementState::Pressed {
        return;
    }

    if let PhysicalKey::Code(code) = event.physical_key {
        if app.modifiers.control_key() {
            match code {
                KeyCode::KeyC => {
                    app.pty.add_bytes(&[3]);
                    return;
                }
                KeyCode::KeyA => {
                    app.pty.add_bytes(&[0x01]);
                    return;
                }
                KeyCode::KeyB => {
                    app.pty.add_bytes(&[0x02]);
                    return;
                }
                KeyCode::KeyD => {
                    app.pty.add_bytes(&[0x04]);
                    return;
                }
                KeyCode::KeyE => {
                    app.pty.add_bytes(&[0x05]);
                    return;
                }
                KeyCode::KeyF => {
                    app.pty.add_bytes(&[0x06]);
                    return;
                }
                KeyCode::KeyG => {
                    app.pty.add_bytes(&[0x07]);
                    return;
                }
                KeyCode::KeyH => {
                    app.pty.add_bytes(&[0x08]);
                    return;
                }
                KeyCode::KeyI => {
                    app.pty.add_bytes(&[0x09]);
                    return;
                }
                KeyCode::KeyJ => {
                    app.pty.add_bytes(&[0x0a]);
                    return;
                }
                KeyCode::KeyK => {
                    app.pty.add_bytes(&[0x0b]);
                    return;
                }
                KeyCode::KeyL => {
                    app.pty.add_bytes(&[0x0c]);
                    return;
                }
                KeyCode::KeyM => {
                    app.pty.add_bytes(&[0x0d]);
                    return;
                }
                KeyCode::KeyN => {
                    app.pty.add_bytes(&[0x0e]);
                    return;
                }
                KeyCode::KeyO => {
                    app.pty.add_bytes(&[0x0f]);
                    return;
                }
                KeyCode::KeyP => {
                    app.pty.add_bytes(&[0x10]);
                    return;
                }
                KeyCode::KeyQ => {
                    app.pty.add_bytes(&[0x11]);
                    return;
                }
                KeyCode::KeyR => {
                    app.pty.add_bytes(&[0x12]);
                    return;
                }
                KeyCode::KeyS => {
                    app.pty.add_bytes(&[0x13]);
                    return;
                }
                KeyCode::KeyT => {
                    app.pty.add_bytes(&[0x14]);
                    return;
                }
                KeyCode::KeyU => {
                    app.pty.add_bytes(&[0x15]);
                    return;
                }
                KeyCode::KeyV => {
                    app.pty.add_bytes(&[0x16]);
                    return;
                }
                KeyCode::KeyW => {
                    app.pty.add_bytes(&[0x17]);
                    return;
                }
                KeyCode::KeyX => {
                    app.pty.add_bytes(&[0x18]);
                    return;
                }
                KeyCode::KeyY => {
                    app.pty.add_bytes(&[0x19]);
                    return;
                }
                KeyCode::KeyZ => {
                    app.pty.add_bytes(&[0x1a]);
                    return;
                }
                _ => {}
            }
        }
    }
    let is_csi = app.is_csi();
    let is_app_cursor_mode = app.vt.screen().application_cursor();
    let is_keypad_mode = app.vt.screen().application_keypad();
    match &event.logical_key {
        Key::Named(NamedKey::Escape) => app.pty.add_bytes([0x1b]),
        Key::Named(NamedKey::Enter) => app.pty.add_bytes(b"\r"),
        Key::Named(NamedKey::Backspace) => app.pty.add_bytes([0x7f]),
        Key::Named(NamedKey::Tab) => {
            if app.modifiers.shift_key() {
                app.pty.add_bytes(b"\x1b[Z");
            } else {
                app.pty.add_bytes(b"\t");
            }
        }
        Key::Named(NamedKey::ArrowUp) => app.pty.add_cursor_key(is_csi, b'A', is_app_cursor_mode),
        Key::Named(NamedKey::ArrowDown) => app.pty.add_cursor_key(is_csi, b'B', is_app_cursor_mode),
        Key::Named(NamedKey::ArrowRight) => {
            app.pty.add_cursor_key(is_csi, b'C', is_app_cursor_mode)
        }
        Key::Named(NamedKey::ArrowLeft) => app.pty.add_cursor_key(is_csi, b'D', is_app_cursor_mode),
        Key::Named(NamedKey::Home) => app.pty.add_csi_key(is_csi, b'H'),
        Key::Named(NamedKey::End) => app.pty.add_csi_key(is_csi, b'F'),
        Key::Named(NamedKey::Insert) => app.pty.add_csi_tilde(is_csi, 2),
        Key::Named(NamedKey::Delete) => app.pty.add_csi_tilde(is_csi, 3),
        Key::Named(NamedKey::PageUp) => app.pty.add_csi_tilde(is_csi, 5),
        Key::Named(NamedKey::PageDown) => app.pty.add_csi_tilde(is_csi, 6),
        Key::Named(NamedKey::F1) => {
            if let Some(m) = is_csi {
                app.pty.add_bytes(format!("\x1b[1;{}P", m).as_bytes());
            } else {
                app.pty.add_bytes(b"\x1bOP");
            }
        }
        Key::Named(NamedKey::F2) => {
            if let Some(m) = is_csi {
                app.pty.add_bytes(format!("\x1b[1;{}Q", m).as_bytes());
            } else {
                app.pty.add_bytes(b"\x1bOQ");
            }
        }
        Key::Named(NamedKey::F3) => {
            if let Some(m) = is_csi {
                app.pty.add_bytes(format!("\x1b[1;{}R", m).as_bytes());
            } else {
                app.pty.add_bytes(b"\x1bOR");
            }
        }
        Key::Named(NamedKey::F4) => {
            if let Some(m) = is_csi {
                app.pty.add_bytes(format!("\x1b[1;{}S", m).as_bytes());
            } else {
                app.pty.add_bytes(b"\x1bOS");
            }
        }
        Key::Named(NamedKey::F5) => app.pty.add_csi_tilde(is_csi, 15),
        Key::Named(NamedKey::F6) => app.pty.add_csi_tilde(is_csi, 17),
        Key::Named(NamedKey::F7) => app.pty.add_csi_tilde(is_csi, 18),
        Key::Named(NamedKey::F8) => app.pty.add_csi_tilde(is_csi, 19),
        Key::Named(NamedKey::F9) => app.pty.add_csi_tilde(is_csi, 20),
        Key::Named(NamedKey::F10) => app.pty.add_csi_tilde(is_csi, 21),
        Key::Named(NamedKey::F11) => app.pty.add_csi_tilde(is_csi, 23),
        Key::Named(NamedKey::F12) => app.pty.add_csi_tilde(is_csi, 24),
        _ => {
            if let Some(text) = &event.text {
                if app.modifiers.alt_key() {
                    app.pty.add_bytes([0x1b]);
                }
                app.pty.add_bytes(text.as_bytes());
            }
        }
    }
}
