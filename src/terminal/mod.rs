pub mod manager;
use crate::{puty::Pty, CursorState};
pub use manager::*;
use winit::{
    event::{ElementState, KeyEvent},
    keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey},
};

pub enum SplitDirection {
    Horizontal,
    Vertical,
}

pub struct Pane {
    session: SessionId,
    relative_size: [f32; 2],
    split: Option<(SplitDirection, Box<Self>)>,
}

impl Pane {
    pub fn new(session: SessionId) -> Self {
        Self {
            session,
            relative_size: [1.0, 1.0],
            split: None,
        }
    }

    pub fn split(&mut self, session: SessionId) {
        self.relative_size = [0.5, 1.0];
        self.split = Some((SplitDirection::Horizontal, Box::new(Self::new(session))));
    }

    pub fn sessions(&self) -> Vec<SessionId> {
        let mut sessions = Vec::new();
        self.sessions_internal(&mut sessions);
        sessions
    }

    fn sessions_internal(&self, sessions: &mut Vec<SessionId>) {
        sessions.push(self.session);
        if let Some((_, pane)) = self.split.as_ref() {
            pane.sessions_internal(sessions);
        }
    }
}

pub struct TerminalSession {
    pub _id: SessionId,
    pub pty: Pty,
    pub vt: vt100::Parser,
    pub cursor_style: CursorState,
    //pub rows: u16,
    //pub cols: u16,
}

impl TerminalSession {
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

    pub fn handle_key_press(&mut self, event: &KeyEvent, mods: ModifiersState, is_csi: Option<u8>) {
        if event.state != ElementState::Pressed {
            return;
        }

        if let PhysicalKey::Code(code) = event.physical_key {
            if mods.control_key() {
                match code {
                    KeyCode::KeyC => {
                        self.pty.add_bytes(&[3]);
                        return;
                    }
                    KeyCode::KeyA => {
                        self.pty.add_bytes(&[0x01]);
                        return;
                    }
                    KeyCode::KeyB => {
                        self.pty.add_bytes(&[0x02]);
                        return;
                    }
                    KeyCode::KeyD => {
                        self.pty.add_bytes(&[0x04]);
                        return;
                    }
                    KeyCode::KeyE => {
                        self.pty.add_bytes(&[0x05]);
                        return;
                    }
                    KeyCode::KeyF => {
                        self.pty.add_bytes(&[0x06]);
                        return;
                    }
                    KeyCode::KeyG => {
                        self.pty.add_bytes(&[0x07]);
                        return;
                    }
                    KeyCode::KeyH => {
                        self.pty.add_bytes(&[0x08]);
                        return;
                    }
                    KeyCode::KeyI => {
                        self.pty.add_bytes(&[0x09]);
                        return;
                    }
                    KeyCode::KeyJ => {
                        self.pty.add_bytes(&[0x0a]);
                        return;
                    }
                    KeyCode::KeyK => {
                        self.pty.add_bytes(&[0x0b]);
                        return;
                    }
                    KeyCode::KeyL => {
                        self.pty.add_bytes(&[0x0c]);
                        return;
                    }
                    KeyCode::KeyM => {
                        self.pty.add_bytes(&[0x0d]);
                        return;
                    }
                    KeyCode::KeyN => {
                        self.pty.add_bytes(&[0x0e]);
                        return;
                    }
                    KeyCode::KeyO => {
                        self.pty.add_bytes(&[0x0f]);
                        return;
                    }
                    KeyCode::KeyP => {
                        self.pty.add_bytes(&[0x10]);
                        return;
                    }
                    KeyCode::KeyQ => {
                        self.pty.add_bytes(&[0x11]);
                        return;
                    }
                    KeyCode::KeyR => {
                        self.pty.add_bytes(&[0x12]);
                        return;
                    }
                    KeyCode::KeyS => {
                        self.pty.add_bytes(&[0x13]);
                        return;
                    }
                    KeyCode::KeyT => {
                        self.pty.add_bytes(&[0x14]);
                        return;
                    }
                    KeyCode::KeyU => {
                        self.pty.add_bytes(&[0x15]);
                        return;
                    }
                    KeyCode::KeyV => {
                        self.pty.add_bytes(&[0x16]);
                        return;
                    }
                    KeyCode::KeyW => {
                        self.pty.add_bytes(&[0x17]);
                        return;
                    }
                    KeyCode::KeyX => {
                        self.pty.add_bytes(&[0x18]);
                        return;
                    }
                    KeyCode::KeyY => {
                        self.pty.add_bytes(&[0x19]);
                        return;
                    }
                    KeyCode::KeyZ => {
                        self.pty.add_bytes(&[0x1a]);
                        return;
                    }
                    _ => {}
                }
            }
        }
        let is_app_cursor_mode = self.vt.screen().application_cursor();
        let _is_keypad_mode = self.vt.screen().application_keypad();
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.pty.add_bytes([0x1b]),
            Key::Named(NamedKey::Enter) => self.pty.add_bytes(b"\r"),
            Key::Named(NamedKey::Backspace) => self.pty.add_bytes([0x7f]),
            Key::Named(NamedKey::Tab) => {
                if mods.shift_key() {
                    self.pty.add_bytes(b"\x1b[Z");
                } else {
                    self.pty.add_bytes(b"\t");
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.pty.add_cursor_key(is_csi, b'A', is_app_cursor_mode)
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.pty.add_cursor_key(is_csi, b'B', is_app_cursor_mode)
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.pty.add_cursor_key(is_csi, b'C', is_app_cursor_mode)
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.pty.add_cursor_key(is_csi, b'D', is_app_cursor_mode)
            }
            Key::Named(NamedKey::Home) => self.pty.add_csi_key(is_csi, b'H'),
            Key::Named(NamedKey::End) => self.pty.add_csi_key(is_csi, b'F'),
            Key::Named(NamedKey::Insert) => self.pty.add_csi_tilde(is_csi, 2),
            Key::Named(NamedKey::Delete) => self.pty.add_csi_tilde(is_csi, 3),
            Key::Named(NamedKey::PageUp) => self.pty.add_csi_tilde(is_csi, 5),
            Key::Named(NamedKey::PageDown) => self.pty.add_csi_tilde(is_csi, 6),
            Key::Named(NamedKey::F1) => {
                if let Some(m) = is_csi {
                    self.pty.add_bytes(format!("\x1b[1;{}P", m).as_bytes());
                } else {
                    self.pty.add_bytes(b"\x1bOP");
                }
            }
            Key::Named(NamedKey::F2) => {
                if let Some(m) = is_csi {
                    self.pty.add_bytes(format!("\x1b[1;{}Q", m).as_bytes());
                } else {
                    self.pty.add_bytes(b"\x1bOQ");
                }
            }
            Key::Named(NamedKey::F3) => {
                if let Some(m) = is_csi {
                    self.pty.add_bytes(format!("\x1b[1;{}R", m).as_bytes());
                } else {
                    self.pty.add_bytes(b"\x1bOR");
                }
            }
            Key::Named(NamedKey::F4) => {
                if let Some(m) = is_csi {
                    self.pty.add_bytes(format!("\x1b[1;{}S", m).as_bytes());
                } else {
                    self.pty.add_bytes(b"\x1bOS");
                }
            }
            Key::Named(NamedKey::F5) => self.pty.add_csi_tilde(is_csi, 15),
            Key::Named(NamedKey::F6) => self.pty.add_csi_tilde(is_csi, 17),
            Key::Named(NamedKey::F7) => self.pty.add_csi_tilde(is_csi, 18),
            Key::Named(NamedKey::F8) => self.pty.add_csi_tilde(is_csi, 19),
            Key::Named(NamedKey::F9) => self.pty.add_csi_tilde(is_csi, 20),
            Key::Named(NamedKey::F10) => self.pty.add_csi_tilde(is_csi, 21),
            Key::Named(NamedKey::F11) => self.pty.add_csi_tilde(is_csi, 23),
            Key::Named(NamedKey::F12) => self.pty.add_csi_tilde(is_csi, 24),
            _ => {
                if let Some(text) = &event.text {
                    if mods.alt_key() {
                        self.pty.add_bytes([0x1b]);
                    }
                    self.pty.add_bytes(text.as_bytes());
                }
            }
        }
    }

    /*pub fn handle_mouse_button(
        &mut self,
        button: MouseButton,
        mods: ModifiersState,
        size: PhysicalSize<u32>,
    ) {
        if self.vt.screen().mouse_protocol_mode() != MouseProtocolMode::None {
            let button_code = match button {
                MouseButton::Left => 0,
                MouseButton::Middle => 1,
                MouseButton::Right => 2,
                _ => return,
            };

            let mut code = button_code;
            if mods.shift_key() {
                code += 4;
            }
            if mods.alt_key() {
                code += 8;
            }
            if mods.control_key() {
                code += 16;
            }
            if action == MouseAction::Drag {
                code += 32;
            }

            // SGR format: different M/m for press vs release
            let terminator = match action {
                MouseAction::Press | MouseAction::Drag => 'M',
                MouseAction::Release => 'm',
                MouseAction::Move => 'm', // Motion without button
            };

            // Coords are 1-indexed in SGR mode
            let seq = format!("\x1b[<{};{};{}{}", code, col + 1, row + 1, terminator);
            self.pty.add_bytes(seq.as_bytes());
        }
    }*/
}
