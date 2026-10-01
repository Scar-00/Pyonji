use anyhow::Result;
use async_channel::Sender;
use mlua::prelude::*;
use std::{collections::HashMap, path::Path};

use crate::{
    pty::{Event, Pty, SshConnection},
    terminal::{CursorState, TerminalSession},
};
use vt100::Callbacks;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromLua for SessionId {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        Ok(Self(u64::from_lua(value, lua)?))
    }
}

/// Parsed from a command line, where a session is written as its bare number.
impl std::str::FromStr for SessionId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.trim().parse()?))
    }
}

impl IntoLua for SessionId {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        self.0.into_lua(lua)
    }
}

pub struct CB {
    title: Option<String>,
    cursor_style: CursorState,
    cursor_blink: bool,
    replies: Vec<Vec<u8>>,
}

impl CB {
    fn parse_title(title: &str) -> Option<String> {
        if let Some((_, program)) = title.split_once('-') {
            Some(program.trim().to_string())
        } else {
            let path = Path::new(title);
            path.file_name().map(|file| file.display().to_string())
        }
    }
}

impl Callbacks for CB {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {}

    fn visual_bell(&mut self, _: &mut vt100::Screen) {}

    fn resize(&mut self, _: &mut vt100::Screen, _request: (u16, u16)) {}

    fn set_window_icon_name(&mut self, _: &mut vt100::Screen, _icon_name: &[u8]) {}

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        let Ok(title) = std::str::from_utf8(title) else {
            return;
        };
        let Some(title) = Self::parse_title(title) else {
            return;
        };
        self.title = Some(title);
    }

    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, _ty: &[u8], _data: &[u8]) {}

    fn paste_from_clipboard(&mut self, _: &mut vt100::Screen, _ty: &[u8]) {}

    fn unhandled_char(&mut self, _: &mut vt100::Screen, _c: char) {}

    fn unhandled_control(&mut self, _: &mut vt100::Screen, _b: u8) {}

    fn unhandled_escape(&mut self, _: &mut vt100::Screen, _: Option<u8>, _: Option<u8>, _: u8) {}

    fn reset(&mut self, _: &mut vt100::Screen) {
        self.cursor_style = CursorState::Block;
        self.cursor_blink = true;
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if i2.is_some() {
            return;
        }
        let param = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, param) {
            (Some(b' '), 'q', 0..=6) => {
                self.cursor_style = match param {
                    0..=2 => CursorState::Block,
                    3..=4 => CursorState::Underline,
                    _ => CursorState::Bar,
                };
                self.cursor_blink = matches!(param, 0 | 1 | 3 | 5);
            }
            (None, 'n', 5) => self.replies.push(b"\x1b[0n".to_vec()),
            (None | Some(b'?'), 'n', 6) => {
                let (row, col) = screen.cursor_position();
                let (_, cols) = screen.size();
                let prefix = if i1.is_some() { "?" } else { "" };
                self.replies.push(
                    format!("\x1b[{prefix}{};{}R", row + 1, col.min(cols - 1) + 1).into_bytes(),
                );
            }
            (None, 'c', 0) => self.replies.push(b"\x1b[?1;2c".to_vec()),
            (Some(b'>'), 'c', 0) => self.replies.push(b"\x1b[>0;3;0c".to_vec()),
            _ => {}
        }
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, _params: &[&[u8]]) {}
}

pub struct SessionManager {
    current_id: u64,
    sessions: HashMap<SessionId, TerminalSession>,
    proxy: Sender<Event>,
}

impl SessionManager {
    pub fn new(proxy: Sender<Event>) -> Self {
        Self {
            current_id: 0,
            sessions: HashMap::new(),
            proxy,
        }
    }

    pub fn create_session(
        &mut self,
        rows: u16,
        cols: u16,
        path: Option<&Path>,
    ) -> Result<SessionId> {
        let id = SessionId(self.current_id);
        self.current_id += 1;
        self.sessions.insert(
            id,
            TerminalSession {
                _id: id,
                pty: Pty::new(rows, cols, self.proxy.clone(), id, path)?,
                vt: vt100::Parser::new_with_callbacks(
                    rows,
                    cols,
                    2000,
                    CB {
                        title: None,
                        cursor_style: CursorState::Block,
                        cursor_blink: true,
                        replies: Vec::new(),
                    },
                ),
                cursor_style: CursorState::Block,
                cursor_blink: true,
                wheel_remainder: 0.0,
                title: "cmd".into(),
                custom_title: None,
                mouse_pressed_button: None,
                last_mouse_cell: None,
            },
        );
        Ok(id)
    }

    #[allow(dead_code)]
    pub fn create_remote_session(
        &mut self,
        rows: u16,
        cols: u16,
        conn: &SshConnection,
    ) -> Result<SessionId> {
        let id = SessionId(self.current_id);
        self.current_id += 1;
        self.sessions.insert(
            id,
            TerminalSession {
                _id: id,
                pty: Pty::new_remote(rows, cols, self.proxy.clone(), id, conn)?,
                vt: vt100::Parser::new_with_callbacks(
                    rows,
                    cols,
                    2000,
                    CB {
                        title: None,
                        cursor_style: CursorState::Block,
                        cursor_blink: true,
                        replies: Vec::new(),
                    },
                ),
                cursor_style: CursorState::Block,
                cursor_blink: true,
                wheel_remainder: 0.0,
                title: "ssh".into(),
                custom_title: None,
                mouse_pressed_button: None,
                last_mouse_cell: None,
            },
        );
        Ok(id)
    }

    pub fn remove_session(&mut self, id: SessionId) {
        self.sessions.remove(&id);
    }

    pub fn update_session(&mut self, id: SessionId, data: &[u8]) {
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        session.vt.process(data);
        let (title, style, blink, replies) = {
            let cb = session.vt.callbacks_mut();
            (
                cb.title.take(),
                cb.cursor_style,
                cb.cursor_blink,
                std::mem::take(&mut cb.replies),
            )
        };
        if let Some(title) = title {
            session.set_title(title);
        }
        session.cursor_style = style;
        session.cursor_blink = blink;
        for reply in replies {
            session.pty.add_bytes(reply);
        }
    }

    pub fn send_text(&mut self, id: SessionId, text: &str) {
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        session.pty.add_bytes(text.as_bytes());
    }

    pub fn session(&self, id: SessionId) -> Option<&TerminalSession> {
        self.sessions.get(&id)
    }

    pub fn session_mut(&mut self, id: SessionId) -> Option<&mut TerminalSession> {
        self.sessions.get_mut(&id)
    }

    pub fn resize_session(&mut self, id: SessionId, rows: u16, cols: u16) {
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        let (rows, cols) = (rows.max(1), cols.max(1));
        if session.vt.screen().size() == (rows, cols) {
            return;
        }
        match session.pty.resize(rows, cols) {
            Ok(()) => session.vt.screen_mut().set_size(rows, cols),
            Err(error) => tracing::error!(%error, %id, rows, cols, "PTY resize failed"),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}
