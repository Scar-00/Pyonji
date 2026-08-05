use ratatui::{prelude::*, widgets::*};
use winit::keyboard::KeyCode;

use crate::{App, terminal::SessionId};

pub struct DetachedView<'a> {
    app: &'a mut App,
}

impl<'a> DetachedView<'a> {
    pub fn new(app: &'a mut App) -> Self {
        Self { app }
    }
}

pub struct DetachedState {
    list_state: ListState,
    rename_target: Option<SessionId>,
}

impl DetachedState {
    pub fn new() -> Self {
        Self {
            list_state: ListState::default(),
            rename_target: None,
        }
    }

    pub fn take_rename_target(&mut self) -> Option<SessionId> {
        self.rename_target.take()
    }

    pub fn handle_events(&mut self, app: &mut App, code: KeyCode) -> bool {
        match code {
            KeyCode::ArrowDown => {
                self.list_state.select_next();
                false
            }
            KeyCode::ArrowUp => {
                self.list_state.select_previous();
                false
            }
            KeyCode::KeyR => {
                let Some(selected) = self.list_state.selected() else {
                    return false;
                };
                let sessions = app.live_detached_sessions();
                self.rename_target = sessions.get(selected).copied();
                false
            }
            KeyCode::Enter
            | KeyCode::Digit1
            | KeyCode::Digit2
            | KeyCode::Digit3
            | KeyCode::Digit4
            | KeyCode::Digit5
            | KeyCode::Digit6
            | KeyCode::Digit7
            | KeyCode::Digit8
            | KeyCode::Digit9 => {
                let Some(selected) = self.list_state.selected() else {
                    return false;
                };
                let sessions = app.live_detached_sessions();
                let Some(session) = sessions.get(selected).copied() else {
                    return false;
                };
                let target = match code {
                    KeyCode::Digit1 => 0usize,
                    KeyCode::Digit2 => 1,
                    KeyCode::Digit3 => 2,
                    KeyCode::Digit4 => 3,
                    KeyCode::Digit5 => 4,
                    KeyCode::Digit6 => 5,
                    KeyCode::Digit7 => 6,
                    KeyCode::Digit8 => 7,
                    KeyCode::Digit9 => 8,
                    _ => app.current_tab,
                };
                app.reattach_session(session, target)
            }
            _ => false,
        }
    }
}

impl StatefulWidget for DetachedView<'_> {
    type State = DetachedState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let list_items = self
            .app
            .live_detached_sessions()
            .into_iter()
            .filter_map(|id| {
                let session = self.app.session_manager.session(id)?;
                Some(Line::from(format!("{id}    {}", session.title())))
            });
        let list = List::new(list_items).highlight_style(Modifier::REVERSED);

        StatefulWidget::render(list, area, buf, &mut state.list_state);
    }
}
