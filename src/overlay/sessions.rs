use ratatui::{prelude::*, widgets::*};
use winit::keyboard::KeyCode;

use crate::{App};

pub struct SessionsView<'a> {
    app: &'a mut App,
}

impl<'a> SessionsView<'a> {
    pub fn new(app: &'a mut App) -> Self {
        Self { app }
    }
}

pub struct SessionsState {
    list_state: ListState,
}

impl SessionsState {
    pub fn new() -> Self {
        Self {
            list_state: ListState::default(),
        }
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
            KeyCode::Enter => {
                if let Some(selected) = self.list_state.selected() {
                    let sessions = app
                        .tabs
                        .iter()
                        .flatten()
                        .flat_map(|tab| tab.sessions())
                        .collect::<Vec<_>>();
                    let session = sessions[selected];
                    if let Some(tab) = app.tabs.iter().position(|tab| {
                        let Some(tab) = tab else {
                            return false;
                        };
                        tab.sessions().contains(&session)
                    }) && tab != app.current_tab
                    {
                        app.switch_tab(tab);
                    }
                    app.set_active_session(session);
                    true
                }else {
                    false
                }
            }
            _ => false
        }
    }
}

impl StatefulWidget for SessionsView<'_> {
    type State = SessionsState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let list_items = self
            .app
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(i, tab)| tab.as_ref().map(|tab| (i, tab)))
            .flat_map(|(i, tab)| {
                tab.sessions()
                    .iter()
                    .filter_map(|id| {
                        let session = self.app.session_manager.session(*id)?;
                        Some(Line::from(format!("[{i}]-{id}    {}", session.title())))
                    })
                    .collect::<Vec<_>>()
            });
        let list = List::new(list_items).highlight_style(Modifier::REVERSED);

        StatefulWidget::render(list, area, buf, &mut state.list_state);
    }
}
