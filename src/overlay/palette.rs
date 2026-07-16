use std::rc::Rc;

use nucleo_matcher::{Matcher, Utf32Str};
use ratatui::{prelude::*, widgets::*};
use ratatui_textarea::{CursorMove, Input, Key, TextArea};
use winit::{event::KeyEvent, keyboard::KeyCode};

use crate::{overlay::Overlay, App};

#[derive(Default)]
pub struct CmdPalleteView;

impl CmdPalleteView {
    fn render_text_area(text_area: &mut TextArea<'static>, area: Rect, buf: &mut Buffer) {
        text_area.set_block(Block::default().borders(Borders::BOTTOM));
        text_area.render(area, buf);
        /*let query = text_area.lines().join("\n");
        let width = query.width();
        let ghost_start = area.x + width as u16 + 1;
        let ghost_area = Rect {
            x: ghost_start,
            y: area.y,
            width: area.width - ghost_start,
            height: 1,
        };
        let ghost = Span::styled("test", Style::default().fg(Color::DarkGray));
        ghost.render(ghost_area, buf);*/
    }
}

pub struct CmdPalleteState {
    commands: Vec<Cmd>,
    filtered: Vec<Cmd>,
    matcher: Matcher,

    text_area: TextArea<'static>,
    list_state: ListState,
}

impl CmdPalleteState {
    pub fn new(cmds: impl IntoIterator<Item = Cmd>) -> Self {
        let cmds = cmds.into_iter().collect::<Vec<_>>();
        Self {
            commands: cmds.clone(),
            filtered: cmds,
            matcher: Matcher::default(),

            text_area: TextArea::default(),
            list_state: ListState::default(),
        }
    }

    pub fn handle_events(
        &mut self,
        _: &mut App,
        code: KeyCode,
        event: &KeyEvent,
    ) -> Option<Rc<dyn Fn(&mut Overlay, &mut App)>> {
        match code {
            KeyCode::ArrowDown => {
                self.list_state.select_next();
            }
            KeyCode::ArrowUp => {
                self.list_state.select_previous();
            }
            KeyCode::Enter => {
                if let Some(selected) = self.list_state.selected() {
                    let action = self.filtered[selected].action.clone();
                    let input = self.text_area.lines().join("\n");
                    let mut split = input.split(' ');
                    split.next();
                    let args = split.map(str::to_string).collect::<Vec<_>>();
                    self.text_area.move_cursor(CursorMove::Head);
                    self.text_area.delete_line_by_end();
                    self.list_state.select_first();
                    return Some(Rc::new(move |overlay, app| {
                        action(overlay, app, args.clone());
                    }));
                }
            }
            KeyCode::Backspace => {
                self.text_area.input(Input {
                    key: Key::Backspace,
                    ..Default::default()
                });
                self.filtered = self.filter_items();
                self.list_state.select_first();
            }
            KeyCode::Tab => {
                let selected = self.list_state.selected()?;
                self.text_area.move_cursor(CursorMove::Head);
                self.text_area.delete_line_by_end();
                self.text_area.insert_str(&self.filtered[selected].name);
            }
            _ => {
                if let Some(text) = &event.text {
                    self.text_area.insert_str(text.as_str());
                    self.filtered = self.filter_items();
                    self.list_state.select_first();
                }
            }
        }
        None
    }

    fn filter_items(&mut self) -> Vec<Cmd> {
        let mut buf_1 = vec![];
        let mut buf_2 = vec![];
        let query = self.text_area.lines().join("\n");
        let (name, _) = query.split_once(' ').unwrap_or((query.as_ref(), ""));
        let mut scores = self
            .commands
            .iter()
            .filter_map(|cmd| {
                Some((
                    cmd,
                    self.matcher.fuzzy_match(
                        Utf32Str::new(&cmd.name, &mut buf_1),
                        Utf32Str::new(name, &mut buf_2),
                    )?,
                ))
            })
            .collect::<Vec<_>>();
        scores.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        scores.into_iter().map(|app| app.0).cloned().collect()
    }
}

impl StatefulWidget for CmdPalleteView {
    type State = CmdPalleteState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let layout =
            Layout::default().constraints([Constraint::Length(2), Constraint::Percentage(100)]);
        let [text, rest] = layout.areas(area);
        Self::render_text_area(&mut state.text_area, text, buf);
        let list = List::new(state.filtered.iter().map(|cmd| {
            let mut line = Line::from(cmd.name.clone());
            cmd.args.iter().for_each(|arg| {
                let span = Span::styled(
                    format!("<{}>", arg.placeholder),
                    Style::default().fg(Color::DarkGray),
                );
                line.push_span(" ");
                line.push_span(span);
            });
            line
        }))
        .highlight_style(Modifier::REVERSED);
        StatefulWidget::render(list, rest, buf, &mut state.list_state);
    }
}

#[derive(Clone)]
pub struct Arg {
    placeholder: &'static str,
}

impl Arg {
    pub fn new(n: &'static str) -> Self {
        Self { placeholder: n }
    }
}

#[derive(Clone)]
pub struct Cmd {
    name: String,
    args: Vec<Arg>,
    action: Rc<dyn Fn(&mut Overlay, &mut App, Vec<String>)>,
}

impl Cmd {
    pub fn new<const N: usize>(
        name: impl ToString,
        args: [Arg; N],
        f: impl 'static + Fn(&mut Overlay, &mut App, [String; N]),
    ) -> Self {
        Self {
            name: name.to_string(),
            args: args.into_iter().collect(),
            action: Rc::new(move |overlay, app, args| {
                let Ok(args) = args.try_into() else {
                    return;
                };
                f(overlay, app, args);
            }),
        }
    }
}
