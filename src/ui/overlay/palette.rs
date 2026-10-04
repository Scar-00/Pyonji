//! The command palette: a command line with the matching commands under it.
//!
//! The line is one buffer. Everything before the first space names the command
//! and filters the list; everything after it is handed to the command
//! untouched. That is the same grammar the status bar's `:` prompt takes, so
//! what is learned in one works in the other.

use std::ops::Range;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, KeyBinding, Rgba, ScrollStrategy,
    SharedString, UniformListScrollHandle, WeakEntity, Window, actions, div, prelude::*, px, rgba,
    uniform_list,
};
use gpui_base::{actions::Cancel, h_flex, v_flex};
use gpui_component::{
    WindowExt,
    input::{Input, InputEvent, InputState},
};

use crate::{
    Next, Prev, PyTheme as _, Pyonji, Submit,
    commands::{self, Origin},
    util,
};

actions!(palette, [Complete]);

const CONTEXT: &str = "CommandPalette";

const ROW: f32 = 46.0;
const SECTION: f32 = 46.0;
const LINE: f32 = 52.0;
const FOOTER: f32 = 40.0;

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", Next, Some(CONTEXT)),
        KeyBinding::new("up", Prev, Some(CONTEXT)),
        KeyBinding::new("enter", Submit, Some(CONTEXT)),
        KeyBinding::new("tab", Complete, Some(CONTEXT)),
    ]);
}

/// A row of the list. A section heading is a row of its own, so it takes up
/// space in the scroll handle and the selection has to step over it.
enum Row {
    Section(Origin),
    Command(usize),
}

/// The names tab walks through, and the one of them now on the line.
///
/// The list is captured from the fragment as it was typed. Once a completion is
/// on the line that fragment is a whole command name, and filtering the table
/// by it again finds only that name — so a walk re-derived per press would
/// never get past the first entry.
#[derive(Default)]
struct Completion {
    names: Vec<SharedString>,
    at: Option<usize>,
}

pub struct PaletteView {
    pyonji: WeakEntity<Pyonji>,

    focus_handle: FocusHandle,
    query: Entity<InputState>,
    scroll_handle: UniformListScrollHandle,

    /// The name of the selected command. A name rather than a row number: the
    /// list is re-filtered on every keystroke, and a number carried across that
    /// would go on pointing at whatever row happens to sit there afterwards.
    selected: Option<SharedString>,
    completion: Completion,
}

impl PaletteView {
    pub fn new(pyonji: &WeakEntity<Pyonji>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Run a command, or a few letters of one")
        });

        let focus_handle = cx.focus_handle();
        cx.on_focus_in(&focus_handle, window, |this, window, cx| {
            this.select_first(cx);
            let focus = this.query.focus_handle(cx).clone();
            window.defer(cx, move |window, cx| {
                window.focus(&focus, cx);
            });
        })
        .detach();

        cx.subscribe(&query, |this, _, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            // A keystroke abandons a walk: the next tab is about whatever is
            // on the line now.
            this.completion = Completion::default();
            this.select_first(cx);
        })
        .detach();

        Self {
            pyonji: pyonji.clone(),

            focus_handle,
            query,
            scroll_handle: UniformListScrollHandle::new(),

            selected: None,
            completion: Completion::default(),
        }
    }

    fn line(&self, cx: &App) -> String {
        self.query.read(cx).value().to_string()
    }

    fn matches(&self, cx: &App) -> Vec<commands::Match> {
        let cmds = commands::all(util::read!(self.pyonji, cx));
        commands::filter(&cmds, &self.line(cx))
    }

    fn rows(matches: &[commands::Match]) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut last: Option<Origin> = None;
        for (ix, entry) in matches.iter().enumerate() {
            if last != Some(entry.command.origin) {
                rows.push(Row::Section(entry.command.origin));
                last = Some(entry.command.origin);
            }
            rows.push(Row::Command(ix));
        }
        rows
    }

    /// Put the highlight on a row, by the name that row answers to.
    fn select(&mut self, name: &str, cx: &mut Context<Self>) {
        self.selected = Some(name.to_string().into());
        cx.notify();
    }

    /// The row the highlight is on, in the list as it stands right now. The
    /// only place a selection is turned into a row, so the view, the footer and
    /// the run cannot each read the list differently.
    fn selected(&self, matches: &[commands::Match]) -> Option<usize> {
        let name = self.selected.as_deref()?;
        matches.iter().position(|entry| entry.command.name == name)
    }

    /// Put the highlight on the best match — where an empty list starts, and
    /// where every new query starts again.
    fn select_first(&mut self, cx: &mut Context<Self>) {
        let matches = self.matches(cx);
        self.selected = matches
            .first()
            .map(|entry| entry.command.name.clone().into());
        if self.selected.is_some() {
            self.scroll_to(0, &matches);
        }
        cx.notify();
    }

    /// Move the highlight `delta` rows, stopping at the ends rather than
    /// wrapping round. A list that silently starts over at the top is a list
    /// that runs a command nobody was looking at.
    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let matches = self.matches(cx);
        if matches.is_empty() {
            self.selected = None;
            return cx.notify();
        }
        let last = matches.len() - 1;
        let at = match self.selected(&matches) {
            // Nothing is on the list yet, so the first press picks where to
            // start: the top going down, the bottom going up.
            None if delta < 0 => last,
            None => 0,
            Some(ix) => (ix as isize + delta).clamp(0, last as isize) as usize,
        };
        self.selected = Some(matches[at].command.name.clone().into());
        self.scroll_to(at, &matches);
        cx.notify();
    }

    /// Bring a row into view. `Nearest` rather than `Center`, so the list holds
    /// still until the highlight would otherwise have left it.
    fn scroll_to(&self, at: usize, matches: &[commands::Match]) {
        // The scroll handle counts the children of the scrolling element, and
        // a heading is a child like any other row.
        let row = Self::rows(matches)
            .iter()
            .position(|row| matches!(row, Row::Command(ix) if *ix == at))
            .unwrap_or(at);
        self.scroll_handle
            .scroll_to_item(row, ScrollStrategy::Nearest);
    }

    fn on_next(&mut self, _: &Next, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    fn on_prev(&mut self, _: &Prev, _: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    /// Put `value` on the line, and put the highlight on what the line now
    /// names.
    ///
    /// A value written from here raises no `InputEvent::Change` — the input
    /// suppresses its events across `set_value` — so nothing that watches for
    /// keystrokes would move the selection. Left alone, the highlight would go
    /// on naming whatever the line said before, and the row under the cursor
    /// and the command on the line would be two different commands.
    fn write_line(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        if value == self.line(cx) {
            return;
        }
        self.query
            .update(cx, |query, cx| query.set_value(value, window, cx));

        // What the line names, and failing that the best match: a line that
        // names nothing on the list — an emptied one, or a name that is not a
        // command — still wants a row under the cursor.
        let matches = self.matches(cx);
        let line = self.line(cx);
        let name = commands::query_name(&line);
        self.selected = matches
            .iter()
            .find(|entry| entry.command.name == name)
            .or_else(|| matches.first())
            .map(|entry| entry.command.name.clone().into());
        cx.notify();
    }

    /// Escape peels one layer at a time, the way it does on the release
    /// screen: first the arguments, then the filter, and only then does it fall
    /// through to the dialog, which closes.
    ///
    /// The dialog raises `Cancel` from its own binding and actions bubble
    /// upwards, so this runs first. Stopping the event is what keeps a cleared
    /// line from also closing the screen.
    fn on_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        let line = self.line(cx);
        let name = commands::query_name(&line).to_string();
        let keep = if commands::query_args(&line).is_empty() {
            String::new()
        } else {
            name
        };
        if keep == line.trim() {
            return cx.propagate();
        }
        self.completion = Completion::default();
        self.write_line(keep, window, cx);
        cx.stop_propagation();
    }

    /// Tab walks the commands the fragment could have meant, best first, and
    /// keeps walking for as long as tab is pressed. It stops at the end of the
    /// list rather than starting over, so a held-down tab lands somewhere
    /// decided instead of somewhere arbitrary.
    fn on_complete(&mut self, _: &Complete, window: &mut Window, cx: &mut Context<Self>) {
        let line = self.line(cx);
        let (name, rest) = commands::split_completion(&line);

        // The line still sitting on the name the last press wrote is what says
        // the walk is the same one. Anything else means a fragment has been
        // typed since, and the walk starts again from that.
        let walking = self
            .completion
            .at
            .and_then(|at| self.completion.names.get(at))
            .is_some_and(|offered| offered.as_ref() == name);

        if !walking {
            let cmds = commands::all(util::read!(self.pyonji, cx));
            // Whether there is anything to add at all, which is not the same
            // question as what to add: the line may already name a command.
            if commands::completion(&cmds, &line, 0).is_none() {
                self.completion = Completion::default();
                return;
            }
            self.completion = Completion {
                names: commands::filter(&cmds, name)
                    .into_iter()
                    .map(|entry| SharedString::from(entry.command.name))
                    .collect(),
                at: None,
            };
        }

        let next = self.completion.at.unwrap_or(0) + usize::from(walking);
        let Some(offered) = self.completion.names.get(next).cloned() else {
            // Off the end of the walk. Drop it, so the next tab starts from
            // whatever is on the line then.
            self.completion = Completion::default();
            return;
        };
        self.completion.at = Some(next);
        // The arguments already typed stay: tab completes the name, and nothing
        // else on the line.
        self.write_line(
            format!("{offered} {rest}").trim_end().to_string(),
            window,
            cx,
        );
    }

    fn on_submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        let matches = self.matches(cx);
        let Some(at) = self.selected(&matches) else {
            return;
        };
        let command = matches[at].command.clone();
        let args = commands::query_args(&self.line(cx));

        // Close first. A command that opens another screen must not stack on
        // top of this one, and focus belongs back on the terminal before the
        // command runs.
        window.close_dialog(cx);
        // Deferred, because this runs inside this view's own update and a
        // command that opens a screen updates it again — on both of them at
        // once. Off the end of this update, that is not a re-entry.
        let pyonji = self.pyonji.clone();
        window.defer(cx, move |window, cx| {
            _ = pyonji.update(cx, |py, cx| {
                command.run(&args, py, window, cx);
                // A command that opened a screen has already claimed focus;
                // only hand it back to the terminal when nothing else wanted
                // it.
                if !window.has_active_dialog(cx) {
                    window.focus(&py.focus_handle, cx);
                }
            });
        });
    }

    /// Put `name` on the line in place of whatever is there.
    fn adopt(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.completion = Completion::default();
        self.write_line(name.to_string(), window, cx);
    }

    // ---------------------------------------------------------------- render

    /// The line the whole screen exists to write.
    fn render_line(&self, shown: usize, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let cmds = commands::all(util::read!(self.pyonji, cx));
        let total = cmds.len();
        let naming = commands::naming(&cmds, &self.line(cx));

        h_flex()
            .h(px(LINE))
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme.border)
            .child(
                // The mark a selected row also uses, so the accent means one
                // thing here: this is the live thing. `h_full` because the row
                // centres its children, and a rule with no height of its own
                // would collapse to nothing.
                div().w(px(2.0)).h_full().flex_shrink_0().bg(if naming {
                    theme.accent
                } else {
                    none()
                }),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .font_family(mono(cx))
                            .text_size(px(15.0))
                            .text_color(theme.text)
                            .child(
                                Input::new(&self.query)
                                    .role(gpui_base::RoleOverride::Presentational)
                                    .flex_1()
                                    .bordered(false)
                                    .appearance(false),
                            ),
                    )
                    // How much of the table the line has narrowed to. With
                    // nothing filtered there is nothing to compare, so the
                    // number only appears once it means something.
                    .when(shown != total, |this| {
                        this.child(
                            h_flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap_1p5()
                                .font_family(mono(cx))
                                .text_size(px(11.0))
                                .text_color(theme.text_muted)
                                .child(format!("{shown}"))
                                .child(div().text_color(theme.border).child("/"))
                                .child(format!("{total}")),
                        )
                    }),
            )
    }

    fn render_section(&self, origin: Origin, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .h(px(SECTION))
            .flex_shrink_0()
            .items_center()
            .pl_3()
            .font_family(mono(cx))
            .text_size(px(10.0))
            .text_color(theme.text_muted)
            .child(origin.label())
    }

    fn render_row(
        &self,
        entry: &commands::Match,
        selected: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let command = &entry.command;
        let name_color = if selected {
            theme.text
        } else {
            theme.text_muted
        };
        let name: SharedString = command.name.clone().into();

        h_flex()
            .id(name.clone())
            .aria_label(format!("Run {}: {}", command.name, command.summary))
            .role(gpui::accesskit::Role::ListBoxOption)
            .aria_selected(selected)
            .when(selected, |row| row.aria_active_descendant())
            .w_full()
            .h(px(ROW))
            .flex_shrink_0()
            .items_center()
            .gap_3()
            .pr_3()
            // A rule and a raised fill, so selection cannot be mistaken for
            // hover.
            .border_l_2()
            .border_color(if selected {
                theme.selected_border
            } else {
                none()
            })
            .when(selected, |this| this.bg(theme.surface_elevated))
            .when(!selected, |this| this.hover(|s| s.bg(theme.hovered)))
            .cursor_pointer()
            .on_click({
                let name = name.clone();
                cx.listener(move |this, _, window, cx| {
                    // Off the same list the highlight is resolved against, so
                    // the row that was clicked and the row that runs are one
                    // row.
                    if this
                        .matches(cx)
                        .iter()
                        .any(|entry| entry.command.name == name)
                    {
                        this.select(&name, cx);
                    }
                    this.on_submit(&Submit, window, cx);
                })
            })
            .child(
                h_flex()
                    .flex_shrink_0()
                    .max_w(px(300.0))
                    .items_center()
                    .gap_2()
                    .pl_3()
                    .font_family(mono(cx))
                    .text_size(px(14.0))
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .font_weight(FontWeight::MEDIUM)
                            .child(marked(
                                &command.name,
                                &entry.marks,
                                if selected {
                                    theme.search_match_active
                                } else {
                                    theme.search_match
                                },
                                name_color,
                            )),
                    )
                    .children(command.args.iter().map(|arg| {
                        h_flex()
                            .h(px(18.0))
                            .px_1()
                            .flex_shrink_0()
                            .items_center()
                            .rounded_sm()
                            .border_1()
                            .border_color(theme.border)
                            .text_size(px(11.0))
                            .text_color(theme.text_muted)
                            .child(format!("<{}>", arg.name))
                    })),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(theme.text_muted)
                    .child(command.summary.clone()),
            )
            // The gutter stays empty until a row is selected, so a mark in it
            // means enter will do this.
            .child(
                h_flex()
                    .w(px(18.0))
                    .flex_shrink_0()
                    .justify_end()
                    .font_family(mono(cx))
                    .text_size(px(12.0))
                    .text_color(if selected { theme.accent } else { none() })
                    .child("\u{23ce}"),
            )
    }

    fn render_list(&self, matches: Vec<commands::Match>, cx: &Context<Self>) -> impl IntoElement {
        let rows = Self::rows(&matches);
        if rows.is_empty() {
            return self.render_empty(cx).into_any_element();
        }
        let selected = self.selected(&matches);

        h_flex()
            .id("palette-list")
            .role(gpui::accesskit::Role::ListBox)
            .aria_label("Matching commands")
            //.flex_1()
            .size_full()
            .py_1()
            //.overflow_y_scroll()
            .child(
                uniform_list(
                    "palette-list-list",
                    rows.len(),
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        rows[range]
                            .iter()
                            .map(|row| match row {
                                Row::Section(origin) => {
                                    this.render_section(*origin, cx).into_any_element()
                                }
                                Row::Command(ix) => this
                                    .render_row(&matches[*ix], selected == Some(*ix), cx)
                                    .into_any_element(),
                            })
                            .collect()
                    }),
                )
                .size_full()
                .track_scroll(&self.scroll_handle),
            )
            .into_any_element()
    }

    /// Nothing matched. Name the word that found nothing, and offer the
    /// command it was probably meant to be.
    fn render_empty(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let name = commands::query_name(&self.line(cx)).to_string();
        let cmds = commands::all(util::read!(self.pyonji, cx));
        let suggestion: Option<SharedString> =
            commands::closest(&cmds, &name).map(|c| c.name.clone().into());
        let heading = if cmds.is_empty() {
            String::from("No commands are registered.")
        } else {
            format!("No command matches \u{201c}{name}\u{201d}.")
        };

        v_flex()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_4()
            .px_8()
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme.text_muted)
                    .child(heading),
            )
            .when_some(suggestion.clone(), |this, suggestion| {
                this.child(
                    h_flex()
                        .id("palette-suggestion")
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(theme.border)
                        .cursor_pointer()
                        .hover(|s| s.border_color(theme.accent))
                        .on_click({
                            let suggestion = suggestion.clone();
                            cx.listener(move |this, _, window, cx| {
                                this.adopt(&suggestion, window, cx)
                            })
                        })
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme.text_muted)
                                .child("Did you mean"),
                        )
                        .child(
                            div()
                                .font_family(mono(cx))
                                .text_size(px(13.0))
                                .text_color(theme.accent)
                                .child(suggestion),
                        ),
                )
            })
    }

    fn render_footer(&self, matches: &[commands::Match], cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let usage = self
            .selected(matches)
            .map(|ix| matches[ix].command.usage())
            .unwrap_or_else(|| String::from("nothing selected"));

        h_flex()
            .min_h(px(FOOTER))
            .py_2()
            .flex_wrap()
            .flex_shrink_0()
            .items_center()
            .gap_4()
            .px_3()
            .border_t_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(mono(cx))
                    .text_size(px(12.0))
                    .text_color(theme.text_muted)
                    .child(usage),
            )
            .child(hint(cx, "\u{2191}\u{2193}", "choose"))
            .child(hint(cx, "\u{23ce}", "run"))
            .child(hint(cx, "tab", "complete"))
            .child(hint(
                cx,
                "esc",
                if !commands::query_args(&self.line(cx)).is_empty() {
                    "clear arguments"
                } else if !self.line(cx).trim().is_empty() {
                    "clear filter"
                } else {
                    "close"
                },
            ))
    }
}

impl Focusable for PaletteView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PaletteView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches(cx);

        v_flex()
            .id("command-palette")
            .key_context(CONTEXT)
            .tab_group()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_next))
            .on_action(cx.listener(Self::on_prev))
            .on_action(cx.listener(Self::on_submit))
            .on_action(cx.listener(Self::on_complete))
            .on_action(cx.listener(Self::on_cancel))
            .size_full()
            .overflow_hidden()
            .child(
                crate::ui::combo_box::editable_combo_box(
                    "command-search-results",
                    "Commands",
                    "Run a command, or a few letters of one",
                    &self.query,
                    true,
                    cx,
                )
                .size_full()
                .min_h_0()
                .child(self.render_line(matches.len(), cx))
                .child(self.render_list(matches.clone(), cx))
                .child(self.render_footer(&matches, cx)),
            )
    }
}

// ------------------------------------------------------------------ helpers

/// A name with the characters the query matched set apart, which is what says
/// *why* a row is on the list.
fn marked(name: &str, marks: &[Range<usize>], mark: Rgba, text: Rgba) -> impl IntoElement {
    let mut mark = mark;
    mark.alpha = 1.0;
    let mut spans: Vec<gpui::Div> = Vec::new();
    let mut at = 0;
    for range in marks {
        if range.start < at {
            continue;
        }
        if range.start > at {
            spans.push(
                div()
                    .text_color(text)
                    .child(name[at..range.start].to_string()),
            );
        }
        spans.push(
            div()
                .text_color(mark)
                .underline()
                .child(name[range.clone()].to_string()),
        );
        at = range.end;
    }
    if at < name.len() {
        spans.push(div().text_color(text).child(name[at..].to_string()));
    }
    h_flex().children(spans)
}

fn hint(cx: &Context<PaletteView>, keys: &'static str, what: &'static str) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .flex_shrink_0()
        .items_center()
        .gap_1p5()
        .child(
            div()
                .font_family(mono(cx))
                .text_size(px(11.0))
                .text_color(theme.text)
                .child(keys),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(theme.text_muted)
                .child(what),
        )
}

fn mono(cx: &App) -> SharedString {
    gpui_component::ActiveTheme::theme(cx)
        .mono_font_family
        .clone()
}

/// A border or fill that should show nothing still needs a colour.
fn none() -> Rgba {
    rgba(0x00000000)
}
