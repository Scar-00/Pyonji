//! The command palette: a command line with the matching commands under it.
//!
//! The line is one buffer. Everything before the first space names the command
//! and filters the list; everything after it is handed to the command
//! untouched. That is the same grammar the status bar's `:` prompt takes, so
//! what is learned in one works in the other.

use std::ops::Range;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, KeyBinding, ListAlignment, ListState,
    Rgba, SharedString, WeakEntity, Window, actions, div, list, prelude::*, px,
};
use gpui_base::{actions::Cancel, h_flex, v_flex};
use gpui_component::{
    Icon, IconName, WindowExt,
    input::{Input, InputEvent, InputState},
};

use crate::{
    Next, Prev, Pyonji, Submit,
    commands::{self, Origin},
    ui::{
        PyTheme as _,
        components::{
            EmptyState, PyInteractive as _, PyStyled as _, counter, editable_combo_box, enter_mark,
            footer_status, hint, panel_footer,
        },
    },
    util,
};

actions!(palette, [Complete]);

const CONTEXT: &str = "CommandPalette";

const ROW: f32 = 34.0;
const SECTION: f32 = 26.0;
const LINE: f32 = 52.0;

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
#[derive(PartialEq, Eq)]
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
    scroll_handle: ListState,
    list_rows: Vec<Row>,

    /// The name of the selected command. A name rather than a row number: the
    /// list is re-filtered on every keystroke, and a number carried across that
    /// would go on pointing at whatever row happens to sit there afterwards.
    selected: Option<SharedString>,
    completion: Completion,
    /// The window is too narrow for a name column and a summary column side by
    /// side; summaries drop the column and keep to one line then.
    compact: bool,
    /// The height this view last measured for itself; see `fit_height`.
    fit: gpui::Pixels,
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
            scroll_handle: ListState::new(0, ListAlignment::Top, px(100.)).measure_all(),
            list_rows: Vec::new(),

            selected: None,
            completion: Completion::default(),
            compact: false,
            fit: gpui::px(520.),
        }
    }

    fn line(&self, cx: &App) -> String {
        self.query.read(cx).value().to_string()
    }

    fn matches(&self, cx: &App) -> Vec<commands::Match> {
        let cmds = commands::all(util::read!(self.pyonji, cx));
        commands::filter(&cmds, &self.line(cx))
    }

    /// The list as drawn: the commands, under a heading per group while
    /// nothing is typed. Once a query ranks them, a heading would split the
    /// ranking, so the list goes flat.
    /// Whether the list is shown in groups: only while nothing is typed.
    fn grouped(&self, cx: &App) -> bool {
        commands::query_name(&self.line(cx)).is_empty()
    }

    fn rows(&self, matches: &[commands::Match], cx: &App) -> Vec<Row> {
        if !self.grouped(cx) {
            return (0..matches.len()).map(Row::Command).collect();
        }
        let mut rows = Vec::new();
        // The first group's label is the one standing above the list (see
        // `render_list`), so the list itself starts with its rows.
        let mut last: Option<Origin> = matches.first().map(|entry| entry.command.origin);
        for (ix, entry) in matches.iter().enumerate() {
            if last != Some(entry.command.origin) {
                rows.push(Row::Section(entry.command.origin));
                last = Some(entry.command.origin);
            }
            rows.push(Row::Command(ix));
        }
        rows
    }

    /// The height measured at the last render. The dialog is built while the
    /// app is mid-update, when this view can't read the app to measure itself.
    pub fn fit_height(&self) -> gpui::Pixels {
        self.fit
    }

    /// Re-measure, and redraw the window once if the dialog has to change size.
    fn update_fit(&mut self, window: &mut Window, cx: &mut App) {
        let fit = self.measure(cx);
        if fit != self.fit {
            self.fit = fit;
            // Refresh is ignored during draw. Schedule it after this frame so
            // a dialog with no focused/blinking input still takes its new size.
            window.defer(cx, |window, _| window.refresh());
        }
    }

    /// How tall the palette needs to be to show every match without blank
    /// space, so a short list gets a short dialog. The dialog caps it.
    fn measure(&self, cx: &App) -> gpui::Pixels {
        let rows = self.rows(&self.matches(cx), cx);
        let list = if rows.is_empty() {
            // The empty state's icon, message, and suggestion or detail.
            170.0
        } else {
            let sections = rows
                .iter()
                .filter(|row| matches!(row, Row::Section(_)))
                .count();
            let commands = rows.len() - sections;
            // Plus the label standing above a grouped list.
            let above = if self.grouped(cx) { SECTION } else { 0.0 };
            above + sections as f32 * SECTION + commands as f32 * ROW + 8.0
        };
        // Input, footer, and the dialog's inner padding and border.
        px((LINE + list + 40.0 + 14.0).ceil())
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
            self.scroll_to(0, &matches, cx);
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
        self.scroll_to(at, &matches, cx);
        cx.notify();
    }

    /// Reveal a row without moving the viewport until selection leaves it.
    fn scroll_to(&self, at: usize, matches: &[commands::Match], cx: &App) {
        // The scroll handle counts the children of the scrolling element, and
        // a heading is a child like any other row.
        let row = self
            .rows(matches, cx)
            .iter()
            .position(|row| matches!(row, Row::Command(ix) if *ix == at))
            .unwrap_or(at);
        if row < self.scroll_handle.item_count() {
            self.scroll_handle.scroll_to_reveal_item(row);
        }
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
    fn render_line(&self, shown: usize, focused: bool, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let cmds = commands::all(util::read!(self.pyonji, cx));
        let total = cmds.len();

        h_flex()
            .h(px(LINE))
            .flex_shrink_0()
            .items_center()
            .gap_3()
            // The icon sits where the rows' names start, like the search
            // fields on the other screens.
            .pl(px(14.0))
            .pr_3()
            .border_b_1()
            // Focus shows on the rule, as on every other screen's field.
            .border_color(if focused {
                theme.focus_ring
            } else {
                theme.border
            })
            .child(
                Icon::new(IconName::Search)
                    .size_4()
                    .flex_shrink_0()
                    .text_color(theme.text_muted),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .mono(cx)
                    .text_size(px(15.0))
                    .text_color(theme.text)
                    .child(
                        Input::new(&self.query)
                            .role(gpui_base::RoleOverride::Presentational)
                            .flex_1()
                            .px_0()
                            .bordered(false)
                            .appearance(false),
                    ),
            )
            // How much of the table the line has narrowed to, counted the
            // way every screen counts.
            .when(total > 0, |this| {
                this.child(counter(shown, total, "command", cx))
            })
    }

    fn render_section(&self, origin: Origin, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        // Sat at the bottom of its slot, so the label reads as belonging to
        // the rows under it rather than floating in a gap.
        h_flex()
            .h(px(SECTION))
            .flex_shrink_0()
            .items_end()
            .pb_1()
            .pl_3()
            .mono(cx)
            .text_size(px(11.0))
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
        // Names are what the eye scans for, so they stay bright on every row;
        // the summary beside them is the muted part.
        let name_color = theme.text;
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
            .list_row(selected, cx)
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
                // A shared minimum width lines the summaries up in one column;
                // only unusually long names and arguments push past it.
                h_flex()
                    .flex_shrink_0()
                    .when(!self.compact, |names| names.min_w(px(210.0)))
                    .max_w(px(300.0))
                    .items_center()
                    .gap_2()
                    .pl_3()
                    .mono(cx)
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
                            // Clear enough to read as "this takes an argument".
                            .border_color(theme.text_disabled)
                            .bg(theme.surface_elevated)
                            .text_size(px(11.0))
                            .text_color(theme.text)
                            .child(format!("<{}>", arg.name))
                    })),
            )
            .child(
                // Two lines rather than one, so a summary keeps its point
                // ("…the focused one") instead of being cut mid-phrase.
                div()
                    .flex_1()
                    .min_w_0()
                    .line_clamp(if self.compact { 1 } else { 2 })
                    .text_ellipsis()
                    .text_size(px(12.0))
                    .line_height(px(15.0))
                    .text_color(theme.text_muted)
                    // A row the name didn't match is there for its summary;
                    // mark the letters there, the way names are marked.
                    .child(
                        gpui::StyledText::new(command.summary.clone()).with_highlights(
                            entry.summary_marks.iter().map(|range| {
                                (
                                    range.clone(),
                                    gpui::HighlightStyle {
                                        color: Some(gpui::rgb_to_hsla({
                                            let mut mark = theme.search_match;
                                            mark.alpha = 1.0;
                                            mark
                                        })),
                                        underline: Some(gpui::UnderlineStyle {
                                            thickness: px(1.0),
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    },
                                )
                            }),
                        ),
                    ),
            )
            .child(enter_mark(selected, cx))
    }

    fn render_list(&self, matches: Vec<commands::Match>, cx: &Context<Self>) -> impl IntoElement {
        let rows = self.rows(&matches, cx);
        if rows.is_empty() {
            return self.render_empty(cx).into_any_element();
        }
        let selected = self.selected(&matches);
        // The group the top of the viewport is in stands above the list, so
        // rows never sit there without saying what they belong to, and
        // nothing scrolls under the label.
        let current = self.grouped(cx).then(|| {
            let top = self.scroll_handle.logical_scroll_top().item_ix;
            rows[..=top.min(rows.len() - 1)]
                .iter()
                .rev()
                .find_map(|row| match row {
                    Row::Section(origin) => Some(*origin),
                    Row::Command(_) => None,
                })
                .or_else(|| matches.first().map(|entry| entry.command.origin))
        });
        let current = current
            .flatten()
            .map(|origin| self.render_section(origin, cx));

        let list = h_flex()
            .id("palette-list")
            .relative()
            .role(gpui::accesskit::Role::ListBox)
            .aria_label("Matching commands")
            //.flex_1()
            .w_full()
            .flex_1()
            .min_h_0()
            .py_1()
            //.overflow_y_scroll()
            .child(crate::ui::components::scroll_viewport(
                list(
                    self.scroll_handle.clone(),
                    cx.processor(move |this, index: usize, _, cx| match &rows[index] {
                        Row::Section(origin) => this.render_section(*origin, cx).into_any_element(),
                        Row::Command(ix) => this
                            .render_row(&matches[*ix], selected == Some(*ix), cx)
                            .into_any_element(),
                    }),
                )
                .size_full(),
                &self.scroll_handle,
            ))
            // The thumb appears on hover/scroll; edge fades cue overflow at rest.
            .child(
                div().absolute().inset_0().child(
                    gpui_base::Scrollbar::new(&self.scroll_handle)
                        .id("palette-list-scrollbar")
                        .axis(gpui_base::ScrollbarAxis::Vertical)
                        .mode(gpui_base::ScrollbarMode::Hover)
                        .viewport_from_layout(),
                ),
            );
        v_flex()
            .size_full()
            .min_h_0()
            .children(current)
            .child(list)
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

        EmptyState::new(heading)
            .nothing(gpui_component::IconName::Search, cx)
            .when(!cmds.is_empty() && suggestion.is_none(), |this| {
                this.detail("Check the spelling, or try fewer letters.")
            })
            .when_some(suggestion.clone(), |this, suggestion| {
                this.child(
                    h_flex()
                        .id("palette-suggestion")
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .outlined(cx)
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
                                .mono(cx)
                                .text_size(px(13.0))
                                .text_color(theme.accent)
                                .child(suggestion),
                        ),
                )
            })
    }

    fn render_footer(&self, matches: &[commands::Match], cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let selected = self.selected(matches);
        // With nothing selected the slot stays empty rather than printing a
        // phrase that looks like a command.
        // The row already shows the name and its placeholders. The footer
        // says what is still missing, or what enter will run.
        let line = self.line(cx);
        let typed = commands::query_args(&line);
        let usage = h_flex().gap_1().items_center().when_some(
            selected
                .map(|ix| &matches[ix].command)
                .filter(|command| !command.args.is_empty()),
            |this, command| {
                if typed.is_empty() {
                    this.child("Type")
                        .child(
                            div().mono(cx).child(
                                command
                                    .args
                                    .iter()
                                    .map(|arg| format!("<{}>", arg.name))
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            ),
                        )
                        .child("after the name")
                } else {
                    this.child("Runs").child(div().mono(cx).child(format!(
                        "{} {}",
                        command.name,
                        typed.join(" ")
                    )))
                }
            },
        );

        panel_footer(cx)
            .px_3()
            .child(footer_status(usage, theme.text_muted))
            .when(selected.is_some() && matches.len() > 1, |this| {
                this.child(hint("↑↓", "choose", cx))
            })
            .when(selected.is_some(), |this| {
                this.child(hint("⏎", "run", cx))
                    .child(hint("tab", "complete", cx))
            })
            .child(hint(
                "esc",
                if !commands::query_args(&self.line(cx)).is_empty() {
                    "clear arguments"
                } else if !self.line(cx).trim().is_empty() {
                    "clear filter"
                } else {
                    "close"
                },
                cx,
            ))
    }
}

impl Focusable for PaletteView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PaletteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches(cx);
        let rows = self.rows(&matches, cx);
        if rows != self.list_rows {
            self.scroll_handle.reset(rows.len());
            self.list_rows = rows;
            if let Some(selected) = self.selected(&matches) {
                self.scroll_to(selected, &matches, cx);
            }
        }
        self.compact = window.viewport_size().width < px(700.0);
        self.update_fit(window, cx);

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
                editable_combo_box(
                    "command-search-results",
                    "Commands",
                    "Run a command, or a few letters of one",
                    &self.query,
                    true,
                    cx,
                )
                .size_full()
                .min_h_0()
                .child(self.render_line(
                    matches.len(),
                    self.query.read(cx).focus_handle(cx).is_focused(window),
                    cx,
                ))
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
