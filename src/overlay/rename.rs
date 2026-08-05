use ratatui::{prelude::*, widgets::*};
use ratatui_textarea::{Input, Key, TextArea};
use winit::{event::KeyEvent, keyboard::KeyCode};

use crate::{
    App,
    terminal::{SessionId, TerminalSession},
};

pub struct RenameView<'a> {
    app: &'a mut App,
}

impl<'a> RenameView<'a> {
    pub fn new(app: &'a mut App) -> Self {
        Self { app }
    }
}

pub struct RenameState {
    text_area: TextArea<'static>,
    initialized: bool,
    target: Option<SessionId>,
}

impl RenameState {
    pub fn new() -> Self {
        Self {
            text_area: TextArea::default(),
            initialized: false,
            target: None,
        }
    }

    pub fn reset(&mut self) {
        self.initialized = false;
        self.target = None;
    }

    pub fn open(&mut self, target: SessionId) {
        self.target = Some(target);
        self.initialized = false;
        self.ensure_initialized();
    }

    pub fn take_target(&mut self) -> Option<SessionId> {
        self.target.take()
    }

    fn target_session<'a>(&self, app: &'a App) -> Option<&'a TerminalSession> {
        self.target
            .or_else(|| app.active_session())
            .and_then(|id| app.session_manager.session(id))
    }

    fn ensure_initialized(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.text_area = TextArea::default();
        self.text_area.set_placeholder_text("new name");
        self.text_area.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title("Name"),
        );
    }

    pub fn handle_events(&mut self, app: &mut App, code: KeyCode, event: &KeyEvent) -> bool {
        self.ensure_initialized();
        match code {
            KeyCode::Enter => {
                let session = self.target.or_else(|| app.active_session());
                let name = self.text_area.lines().join(" ").trim().to_string();
                if !name.is_empty()
                    && let Some(session) = session
                    && let Some(session) = app.session_manager.session_mut(session)
                {
                    session.rename(name);
                }
                app.request_redraw();
                true
            }
            KeyCode::Backspace => {
                self.text_area.input(Input {
                    key: Key::Backspace,
                    ..Default::default()
                });
                false
            }
            _ => {
                if let Some(text) = &event.text {
                    self.text_area.insert_str(text.as_str());
                }
                false
            }
        }
    }
}

impl StatefulWidget for RenameView<'_> {
    type State = RenameState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        state.ensure_initialized();

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(5),
                Constraint::Min(0),
            ])
            .split(area);
        let content = layout[1];

        let [info, input, hint] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .areas(content);

        let info_text = match state.target_session(self.app) {
            Some(session) => format!(
                "Session {} · current name: \"{}\"",
                session._id,
                session.title()
            ),
            None => "no session".to_string(),
        };
        Paragraph::new(info_text)
            .style(Style::default().fg(Color::DarkGray))
            .render(info, buf);

        state.text_area.render(input, buf);

        Paragraph::new("Enter confirm · Esc cancel")
            .style(Style::default().fg(Color::DarkGray))
            .render(hint, buf);
    }
}
