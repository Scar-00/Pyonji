use anyhow::Result;
use std::{collections::HashMap, path::Path};
use winit::event_loop::EventLoopProxy;

use crate::{
    puty::{Event, Pty},
    terminal::{CursorState, TerminalSession},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(u64);

pub struct SessionManager {
    current_id: u64,
    sessions: HashMap<SessionId, TerminalSession>,
    active_session: Option<SessionId>,
    proxy: EventLoopProxy<Event>,
}

impl SessionManager {
    pub fn new(proxy: EventLoopProxy<Event>) -> Self {
        Self {
            current_id: 0,
            sessions: HashMap::new(),
            active_session: None,
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

    pub fn resize_sessions(&mut self, rows: u16, cols: u16) {
        for session in self.sessions.values_mut() {
            session.pty.resize(rows, cols);
            session.vt.set_size(rows, cols);
        }
    }

    pub fn active_session(&self) -> Option<&TerminalSession> {
        self.active_session
            .as_ref()
            .map(|id| self.sessions.get(id))
            .flatten()
    }

    pub fn active_session_mut(&mut self) -> Option<&mut TerminalSession> {
        self.active_session
            .as_mut()
            .map(|id| self.sessions.get_mut(id))
            .flatten()
    }

    /*pub fn sessions(&self, ids: impl IntoIterator<Item = SessionId>) -> Vec<&TerminalSession> {
        let mut sessions = Vec::new();
        for id in ids {
            let Some(session) = self.sessions.get(&id) else {
                continue;
            };
            sessions.push(session);
        }
        sessions
    }*/

    pub fn set_active_session(&mut self, id: SessionId) {
        self.active_session = Some(id);
    }

    pub fn active_session_id(&self) -> Option<SessionId> {
        self.active_session
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}
