use crate::App;
use ratatui::{prelude::*, widgets::*};
use ratatui_explorer::{FileExplorer, FileExplorerBuilder, Input, Theme};
use winit::keyboard::KeyCode;

#[derive(Default)]
pub struct OpenerView;

pub struct OpenerState {
    pub explorer: FileExplorer,
}

impl OpenerState {
    pub fn new() -> Self {
        let theme = Theme::default().with_block(Block::default());
        Self {
            explorer: FileExplorerBuilder::default()
                .filter_map(|file| if file.is_dir { Some(file) } else { None })
                .theme(theme)
                .build()
                .unwrap(),
        }
    }

    pub fn handle_events(&mut self, _: &mut App, code: KeyCode) {
        match code {
            KeyCode::ArrowDown => {
                _ = self.explorer.handle(Input::Down);
            }
            KeyCode::ArrowUp => {
                _ = self.explorer.handle(Input::Up);
            }
            KeyCode::Enter => {
                _ = self.explorer.handle(Input::Right);
            }
            KeyCode::KeyO => {
                _ = self.explorer.set_cwd("/");
            }
            _ => {}
        }
    }
}

impl StatefulWidget for OpenerView {
    type State = OpenerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        state.explorer.widget().render_ref(area, buf);
    }
}
