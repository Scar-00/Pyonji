use egui::*;
use egui_flex::*;
use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config as MatcherConfig, Matcher, Utf32Str,
};
use winit::event_loop::EventLoopProxy;

use crate::pty::Event;

#[derive(Clone)]
pub struct State {
    items: Vec<Cmd>,
    selected: Option<usize>,
    query: String,

    matcher: Matcher,
}

impl State {
    pub fn reset(&mut self) {
        self.selected = None;
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            items: vec![
                Cmd::new("close", UiAction::Close),
                Cmd::new("next", UiAction::Next),
                Cmd::new("prev", UiAction::Prev),
                Cmd::new("new", UiAction::Prev),
            ],
            selected: None,
            query: String::new(),

            matcher: Matcher::new(MatcherConfig::DEFAULT),
        }
    }
}

#[derive(Debug, Clone)]
struct Cmd {
    name: String,
    action: UiAction,
}

impl Cmd {
    pub fn new(name: impl ToString, action: UiAction) -> Self {
        Self {
            name: name.to_string(),
            action,
        }
    }
}

pub struct Palette<'a> {
    state: &'a mut State,
    proxy: &'a EventLoopProxy<Event>,
}

#[derive(Debug, Clone, Copy)]
pub enum UiAction {
    Close,
    Next,
    Prev,
}

impl<'a> Palette<'a> {
    pub const BACKGROUND: Color32 = Color32::from_rgba_unmultiplied_const(0x1e, 0x1e, 0x1e, 0xF0);
    pub const BORDER: Color32 = Color32::from_rgb(0x50, 0x50, 0x50);
    pub const SELECTED: Color32 = Color32::from_rgb(0x30, 0x30, 0x30);

    pub fn new(state: &'a mut State, proxy: &'a EventLoopProxy<Event>) -> Self {
        Self { state, proxy }
    }

    pub fn show(&mut self, ui: &mut Ui) {
        let next = KeyboardShortcut::new(Modifiers::NONE, Key::ArrowDown);
        let prev = KeyboardShortcut::new(Modifiers::NONE, Key::ArrowUp);
        let submit = KeyboardShortcut::new(Modifiers::NONE, Key::Enter);

        ui.input_mut(|input| {
            if input.consume_shortcut(&next) {
                Self::select_next(self.state);
            }
            if input.consume_shortcut(&prev) {
                Self::select_prev(self.state);
            }
            if input.consume_shortcut(&submit) {
                if let Some(selected) = self.state.selected.as_ref() {
                    self.send(self.state.items[*selected].action);
                    self.state.reset();
                }
            }
        });
        let filtered = Self::filtered_items(self.state);
        CentralPanel::no_frame().show(ui, |ui| {
            ui.add_sized(ui.available_size(), |ui: &mut Ui| -> Response {
                let size = ui.available_size() / 2.0;
                Flex::vertical()
                    .justify(FlexJustify::Center)
                    .align_items(FlexAlign::Center)
                    .show(ui, |flex| {
                        flex.add_ui(egui_flex::item().min_size(size), |ui| {
                            Frame::central_panel(&Style::default())
                                .fill(Self::BACKGROUND)
                                .stroke(Stroke::new(1.0, Self::BORDER))
                                .outer_margin(Margin::symmetric(16, 9 * 2))
                                .corner_radius(8.0)
                                .show(ui, |ui| {
                                    ui.vertical(|ui| {
                                        ui.add(
                                            TextEdit::singleline(&mut self.state.query)
                                                .lock_focus(true)
                                                .background_color(Self::BACKGROUND)
                                                .desired_width(ui.available_width() / 4.0)
                                                .font(FontId::proportional(24.0))
                                                .hint_text(RichText::new("Search..").size(24.0)),
                                        )
                                        .request_focus();
                                        ui.add_sized(
                                            [ui.available_width() / 4.0, 32.0],
                                            Separator::default().horizontal(),
                                        );
                                        ScrollArea::vertical().show_rows(
                                            ui,
                                            32.0,
                                            filtered.len(),
                                            |ui, rows| {
                                                filtered[rows].iter().enumerate().for_each(
                                                    |(i, row)| {
                                                        ui.add_sized(
                                                            [ui.available_width() / 4.0, 32.0],
                                                            |ui: &mut Ui| {
                                                                if self.state.selected == Some(i) {
                                                                    ui.painter().rect(
                                                                    ui.available_rect_before_wrap(),
                                                                    4.0,
                                                                    Self::SELECTED,
                                                                    Stroke::NONE,
                                                                    StrokeKind::Middle,
                                                                );
                                                                }
                                                                ui.label(
                                                                    RichText::new(&row.name)
                                                                        .size(18.0),
                                                                )
                                                            },
                                                        );
                                                    },
                                                );
                                            },
                                        );
                                    });
                                });
                        });
                    })
                    .response
            });
        });
    }

    fn filtered_items(state: &mut State) -> Vec<Cmd> {
        let mut buf_1 = vec![];
        let mut buf_2 = vec![];
        let mut scores = state
            .items
            .iter()
            .filter_map(|cmd| {
                Some((
                    cmd,
                    state.matcher.fuzzy_match(
                        Utf32Str::new(&cmd.name, &mut buf_1),
                        Utf32Str::new(&state.query, &mut buf_2),
                    )?,
                ))
            })
            .collect::<Vec<_>>();
        scores.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        scores.into_iter().map(|app| app.0).cloned().collect()
    }

    fn select_next(state: &mut State) {
        if state.selected.is_none() && !state.items.is_empty() {
            state.selected = Some(0);
            return;
        }
        if state.selected == Some(state.items.len() - 1) {
            state.selected = Some(0);
            return;
        }
        state.selected.as_mut().map(|selected| *selected += 1);
    }

    fn select_prev(state: &mut State) {
        if state.selected.is_none() && !state.items.is_empty() {
            state.selected = Some(state.items.len() - 1);
            return;
        }
        if state.selected == Some(0) {
            state.selected = Some(state.items.len() - 1);
            return;
        }
        state.selected.as_mut().map(|selected| *selected -= 1);
    }

    fn send(&self, action: UiAction) {
        _ = self.proxy.send_event(Event::UiAction(action));
    }
}
