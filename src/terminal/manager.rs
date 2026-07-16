use anyhow::Result;
use std::{collections::HashMap, path::Path};
use winit::event_loop::EventLoopProxy;

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

pub struct CB {
    id: SessionId,
    proxy: EventLoopProxy<Event>,
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
        _ = self
            .proxy
            .send_event(Event::ProgramChanged((self.id, title)));
    }

    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, _ty: &[u8], _data: &[u8]) {}

    fn paste_from_clipboard(&mut self, _: &mut vt100::Screen, _ty: &[u8]) {}

    fn unhandled_char(&mut self, _: &mut vt100::Screen, _c: char) {}

    fn unhandled_control(&mut self, _: &mut vt100::Screen, _b: u8) {}

    fn unhandled_escape(&mut self, _: &mut vt100::Screen, _: Option<u8>, _: Option<u8>, _: u8) {}

    fn unhandled_csi(
        &mut self,
        _: &mut vt100::Screen,
        _: Option<u8>,
        _: Option<u8>,
        _: &[&[u16]],
        _: char,
    ) {
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, _params: &[&[u8]]) {}
}

pub struct SessionManager {
    current_id: u64,
    sessions: HashMap<SessionId, TerminalSession>,
    proxy: EventLoopProxy<Event>,
}

impl SessionManager {
    pub fn new(proxy: EventLoopProxy<Event>) -> Self {
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
                        id,
                        proxy: self.proxy.clone(),
                    },
                ),
                cursor_style: CursorState::Bar,
                title: "cmd".into(),
                mouse_pressed_button: None,
                last_mouse_cell: None,
            },
        );
        Ok(id)
    }

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
                        id,
                        proxy: self.proxy.clone(),
                    },
                ),
                cursor_style: CursorState::Bar,
                title: "ssh".into(),
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
        session.interrupt_pty_data(data);
        session.vt.process(data);
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
        session.pty.resize(rows, cols);
        session.vt.screen_mut().set_size(rows, cols);
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}
