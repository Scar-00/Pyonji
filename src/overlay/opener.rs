use std::path::PathBuf;

use crate::App;
use ratatui::{prelude::*, widgets::*};
use ratatui_explorer::{FileExplorer, FileExplorerBuilder, Input, Theme};
use winit::keyboard::KeyCode;

#[derive(Default)]
pub struct OpenerView;

pub struct OpenerState {
    pub explorer: FileExplorer,
    pub selected_path: Option<PathBuf>,
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
            selected_path: None,
        }
    }

    /// Returns `true` if the user confirmed a directory selection.
    pub fn handle_events(&mut self, _: &mut App, code: KeyCode) -> bool {
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
            KeyCode::Space => {
                self.selected_path = Some(self.explorer.cwd().to_path_buf());
            }
            _ => {}
        }
        self.selected_path.is_some()
    }
}

impl StatefulWidget for OpenerView {
    type State = OpenerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        state.explorer.widget().render_ref(area, buf);
    }
}
