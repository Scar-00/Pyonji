use anyhow::Result;
use std::{collections::HashMap, path::Path};
use winit::event_loop::EventLoopProxy;

use crate::{
    pty::{Event, Pty},
    terminal::{CursorState, TerminalSession},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

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
                vt: vt100::Parser::new(rows, cols, 2000),
                cursor_style: CursorState::Bar,
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
        session.interrupt_pty_data(&data);
        session.vt.process(&data);
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
