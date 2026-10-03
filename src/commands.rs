use std::{collections::HashSet, ops::Range, rc::Rc, str::FromStr};

use gpui::{Context, Window};
use nucleo_matcher::{Config as MatchConfig, Matcher, Utf32Str};

use crate::{
    Pyonji,
    config::{self, LuaAction},
    logging::ResultLogExt as _,
    pty::SshConnection,
    terminal::SessionId,
    ui::{OverlayScreen, StatusBarMode},
};

pub type Runner = Rc<dyn Fn(&[String], &mut Pyonji, &mut Window, &mut Context<Pyonji>)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    Workspace,
    Remote,
    Config,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Origin::Workspace => "workspace",
            Origin::Remote => "remote",
            Origin::Config => "from your config",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub name: String,
}

impl Arg {
    pub fn new(name: impl ToString) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct Command {
    pub name: String,
    pub args: Vec<Arg>,
    /// One line, in the imperative, saying what running it does.
    pub summary: String,
    pub origin: Origin,
    run: Runner,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Command")
            .field("name", &self.name)
            .field("args", &self.args)
            .field("summary", &self.summary)
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

impl Command {
    pub fn new(
        name: impl ToString,
        args: impl IntoIterator<Item = Arg>,
        origin: Origin,
        summary: impl ToString,
        run: impl 'static + Fn(&[String], &mut Pyonji, &mut Window, &mut Context<Pyonji>),
    ) -> Self {
        Self {
            name: name.to_string(),
            args: args.into_iter().collect(),
            summary: summary.to_string(),
            origin,
            run: Rc::new(run),
        }
    }

    pub fn run(
        &self,
        args: &[String],
        py: &mut Pyonji,
        window: &mut Window,
        cx: &mut Context<Pyonji>,
    ) {
        (self.run)(args, py, window, cx);
    }

    /// The usage line, e.g. `switch <tab>`.
    pub fn usage(&self) -> String {
        let mut usage = self.name.clone();
        for arg in &self.args {
            usage.push_str(&format!(" <{}>", arg.name));
        }
        usage
    }

    /// The words a query may match against: the name, then the summary, then
    /// the placeholders. In that order, because the name is what the user is
    /// usually typing.
    fn haystacks(&self) -> [String; 3] {
        [
            self.name.clone(),
            self.summary.clone(),
            self.args
                .iter()
                .map(|arg| arg.name.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        ]
    }
}

/// A command that matched a query, with the characters in its *name* the match
/// landed on. A command matched only on its summary carries no marks, which is
/// the point: the underline says the name is what matched.
#[derive(Debug, Clone)]
pub struct Match {
    pub command: Command,
    /// Byte ranges into `command.name`, in order, non-overlapping.
    pub marks: Vec<Range<usize>>,
}

/// Every command available right now.
pub fn all(py: &Pyonji) -> Vec<Command> {
    let mut commands = builtins();
    commands.extend(from_ssh_sessions(&py.ssh_sessions));
    commands.extend(from_lua_actions(&py.registered_callbacks));
    commands
}

pub fn query_name(query: &str) -> &str {
    query.split_whitespace().next().unwrap_or_default()
}

/// The words a query passes to the command it names.
pub fn query_args(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .skip(1)
        .map(str::to_string)
        .collect()
}

/// Fuzzy-filter `commands` by the name part of `query`, best match first.
///
/// Ties keep their declared order, so a group stays in the order the built-in
/// list or the config put it in.
pub fn filter(commands: &[Command], query: &str) -> Vec<Match> {
    let name = query_name(query);
    if name.is_empty() {
        return commands
            .iter()
            .cloned()
            .map(|command| Match {
                command,
                marks: Vec::new(),
            })
            .collect();
    }

    let mut matcher = Matcher::new(MatchConfig::DEFAULT);
    // `sw` should mean `switch`, not some later `show_sessions`.
    matcher.config.prefer_prefix = true;

    let mut needle_buf = Vec::new();
    let mut hay_buf = Vec::new();
    let mut marks: Vec<u32> = Vec::new();
    let mut name_marks: Vec<u32> = Vec::new();

    let mut scored: Vec<(u16, Match)> = commands
        .iter()
        .filter_map(|command| {
            let needle = Utf32Str::new(name, &mut needle_buf);
            let mut best: Option<u16> = None;
            for (ix, haystack) in command.haystacks().iter().enumerate() {
                if haystack.is_empty() {
                    continue;
                }
                // `fuzzy_indices` appends; it never clears for us.
                marks.clear();
                let hay = Utf32Str::new(haystack, &mut hay_buf);
                let Some(score) = matcher.fuzzy_indices(hay, needle, &mut marks) else {
                    continue;
                };
                if ix == 0 {
                    name_marks.clear();
                    name_marks.extend_from_slice(&marks);
                }
                best = Some(best.map_or(score, |top| top.max(score)));
            }
            let score = best?;
            Some((
                score,
                Match {
                    marks: byte_ranges(&command.name, &name_marks),
                    command: command.clone(),
                },
            ))
        })
        .collect();

    // Stable, so equal scores stay in declaration order.
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(_, entry)| entry).collect()
}

/// The character positions the matcher reported, as byte ranges in the same
/// string, with neighbouring characters merged into one run.
///
/// The matcher counts characters, and command names are compared the same way,
/// so the walk is over `char_indices`.
fn byte_ranges(name: &str, marks: &[u32]) -> Vec<Range<usize>> {
    if marks.is_empty() {
        return Vec::new();
    }
    let marked: HashSet<u32> = marks.iter().copied().collect();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (at, (start, ch)) in name.char_indices().enumerate() {
        if !marked.contains(&(at as u32)) {
            continue;
        }
        let end = start + ch.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == start => last.end = end,
            _ => ranges.push(start..end),
        }
    }
    ranges
}

pub fn naming(commands: &[Command], query: &str) -> bool {
    let name = query_name(query);
    if name.is_empty() {
        return false;
    }
    commands.iter().any(|command| {
        command.name.len() >= name.len()
            && command.name.is_char_boundary(name.len())
            && command.name[..name.len()].eq_ignore_ascii_case(name)
    })
}

/// The command a name was probably meant to be.
///
/// Only near misses qualify: one or two edits, allowing for a swapped pair,
/// which is the typo people actually make. A name that matches nothing closely
/// is not a near miss of anything, and guessing anyway would be noise.
pub fn closest<'a>(commands: &'a [Command], name: &str) -> Option<&'a Command> {
    let len = name.chars().count();
    if len < 3 {
        return None;
    }
    let budget = (len / 3).max(1);
    let mut best: Option<(usize, &Command)> = None;
    for command in commands {
        let distance = edit_distance(name, &command.name);
        if distance > budget {
            continue;
        }
        if best.as_ref().is_none_or(|(top, _)| distance < *top) {
            best = Some((distance, command));
        }
    }
    best.map(|(_, command)| command)
}

/// Optimal string alignment distance: insertions, deletions, substitutions, and
/// one swap of neighbours, which plain Levenshtein would charge twice.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut two_back: Vec<usize> = vec![0; b.len() + 1];
    let mut back: Vec<usize> = (0..=b.len()).collect();
    let mut row: Vec<usize> = vec![0; b.len() + 1];

    for i in 1..=a.len() {
        row[0] = i;
        for j in 1..=b.len() {
            let swap = (i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1])
                .then(|| two_back[j - 2] + 1);
            row[j] = (back[j] + 1)
                .min(row[j - 1] + 1)
                .min(back[j - 1] + usize::from(a[i - 1] != b[j - 1]))
                .min(swap.unwrap_or(usize::MAX));
        }
        std::mem::swap(&mut two_back, &mut back);
        std::mem::swap(&mut back, &mut row);
    }
    back[b.len()]
}

/// The line to write when completing `line` against the table, or `None` when
/// there is nothing left to add.
///
/// `step` walks the matches in rank order, so a second press of tab offers the
/// next one. Arguments already typed are kept.
pub fn completion(commands: &[Command], line: &str, step: usize) -> Option<String> {
    let (name, rest) = split_completion(line);
    let matches = filter(commands, name);
    let command = &matches.get(step)?.command;
    if command.name == name {
        return None;
    }
    Some(format!("{} {}", command.name, rest).trim_end().to_string())
}

/// Split a line at the word being typed: the fragment, and everything from the
/// space that follows it onwards.
pub fn split_completion(line: &str) -> (&str, &str) {
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    (&line[..end], line[end..].trim_start())
}

/// A tab is written 1-based on a command line, the way the status bar counts.
fn tab_index(arg: Option<&String>) -> Option<usize> {
    arg?.parse::<usize>().ok()?.checked_sub(1)
}

fn session_id(arg: Option<&String>) -> Option<SessionId> {
    SessionId::from_str(arg?).ok()
}

/// Stand in for opening one of the other screens.
///
/// Going through `Overlay` rather than dispatching an action keeps this the
/// one place that knows a command maps to a screen, and it does not depend on
/// where focus happens to be once the dialog is gone.
fn open(screen: OverlayScreen, py: &mut Pyonji, window: &mut Window, cx: &mut Context<Pyonji>) {
    py.overlay
        .update(cx, |overlay, cx| overlay.open(screen, window, cx));
}

fn builtins() -> Vec<Command> {
    vec![
        Command::new(
            "switch",
            [Arg::new("tab")],
            Origin::Workspace,
            "Go to a tab, opening a session if that slot is empty",
            |args, py: &mut Pyonji, _: &mut Window, cx: &mut Context<Pyonji>| {
                if let Some(tab) = tab_index(args.first())
                    && tab < py.tabs.len()
                {
                    py.switch_tab(tab, cx);
                }
            },
        ),
        Command::new(
            "next-tab",
            [],
            Origin::Workspace,
            "Go to the next tab",
            |_, py: &mut Pyonji, _: &mut Window, cx: &mut Context<Pyonji>| {
                py.next_tab(cx);
            },
        ),
        Command::new(
            "prev-tab",
            [],
            Origin::Workspace,
            "Go to the previous tab",
            |_, py: &mut Pyonji, _: &mut Window, cx: &mut Context<Pyonji>| {
                py.prev_tab(cx);
            },
        ),
        Command::new(
            "close",
            [Arg::new("session")],
            Origin::Workspace,
            "Close a session; leave off the argument to close the focused one",
            |args, py: &mut Pyonji, _: &mut Window, cx: &mut Context<Pyonji>| {
                let Some(id) = session_id(args.first()).or_else(|| py.active_session()) else {
                    return;
                };
                if let Some(session) = py.session_manager.session_mut(id) {
                    session.pty.kill();
                }
                py.close_session(id, cx);
            },
        ),
        Command::new(
            "move-to",
            [Arg::new("tab")],
            Origin::Workspace,
            "Move the focused session into another tab",
            |args, py, _, cx| {
                let (Some(tab), Some(session)) = (tab_index(args.first()), py.active_session())
                else {
                    return;
                };
                py.move_session(None, tab, session, cx);
            },
        ),
        Command::new(
            "split-h",
            [],
            Origin::Workspace,
            "Split the tab into panes side by side",
            |_, py, _, cx| {
                py.split_active(crate::terminal::SplitDirection::Vertical, cx)
                    .log();
            },
        ),
        Command::new(
            "split-v",
            [],
            Origin::Workspace,
            "Split the tab into panes stacked",
            |_, py, _, cx| {
                py.split_active(crate::terminal::SplitDirection::Horizontal, cx)
                    .log();
            },
        ),
        Command::new(
            "focus-next-pane",
            [],
            Origin::Workspace,
            "Move the focus to the next pane",
            |_, py: &mut Pyonji, _: &mut Window, cx: &mut Context<Pyonji>| {
                let Some(current) = py.current_tab else {
                    return;
                };
                py.tabs[current].as_mut().and_then(|tab| tab.focus_next());
                cx.notify();
            },
        ),
        Command::new(
            "detach",
            [],
            Origin::Workspace,
            "Hide the focused session without closing it",
            |_, py, _, cx| {
                let Some(session) = py.active_session() else {
                    return;
                };
                py.detach_session(session, cx);
            },
        ),
        Command::new(
            "rename",
            [Arg::new("name")],
            Origin::Workspace,
            "Rename the focused session",
            |args, py: &mut Pyonji, _: &mut Window, cx: &mut Context<Pyonji>| {
                let Some(session) = py
                    .active_session()
                    .and_then(|id| py.session_manager.session_mut(id))
                else {
                    return;
                };
                session.rename(args.join(" "));
                cx.notify();
            },
        ),
        Command::new(
            "ssh",
            [Arg::new("session")],
            Origin::Workspace,
            "Open an SSH session by the name in the config",
            |args, py: &mut Pyonji, _: &mut Window, cx: &mut Context<Pyonji>| {
                let Some(name) = args.first() else {
                    return;
                };
                let Some(connection) = py
                    .ssh_sessions
                    .iter()
                    .find(|session| session.name == *name)
                    .cloned()
                else {
                    return;
                };
                py.create_remote_session(&connection, None, None, cx).log();
            },
        ),
        Command::new(
            "open-in",
            [],
            Origin::Workspace,
            "Open a file or a directory in a new session",
            |_, py: &mut Pyonji, window: &mut Window, cx: &mut Context<Pyonji>| {
                open(OverlayScreen::Opener, py, window, cx);
            },
        ),
        Command::new(
            "sessions",
            [],
            Origin::Workspace,
            "Attach a detached session or switch to a running session",
            |_, py: &mut Pyonji, window: &mut Window, cx: &mut Context<Pyonji>| {
                open(OverlayScreen::Sessions, py, window, cx);
            },
        ),
        Command::new(
            "detached",
            [],
            Origin::Workspace,
            "Attach a detached session to a tab",
            |_, py: &mut Pyonji, window: &mut Window, cx: &mut Context<Pyonji>| {
                open(OverlayScreen::Detached, py, window, cx);
            },
        ),
        Command::new(
            "releases",
            [],
            Origin::Workspace,
            "Check for a newer Pyonji build",
            |_, py: &mut Pyonji, window: &mut Window, cx: &mut Context<Pyonji>| {
                open(OverlayScreen::Releases, py, window, cx);
            },
        ),
        Command::new(
            "commands",
            [],
            Origin::Workspace,
            "Open the command palette",
            |_, py: &mut Pyonji, window: &mut Window, cx: &mut Context<Pyonji>| {
                open(OverlayScreen::Palette, py, window, cx);
            },
        ),
        Command::new(
            "lua",
            [],
            Origin::Workspace,
            "Run a line of Lua in the status bar",
            |_, py: &mut Pyonji, _: &mut Window, cx: &mut Context<Pyonji>| {
                py.status_bar
                    .update(cx, |bar, cx| bar.set_mode(StatusBarMode::Lua, cx));
            },
        ),
        Command::new(
            "reload-config",
            [],
            Origin::Workspace,
            "Read init.lua from disk again",
            |_, py: &mut Pyonji, window: &mut Window, cx: &mut Context<Pyonji>| {
                config::load(py, window, cx).log();
            },
        ),
    ]
}

fn from_ssh_sessions(sessions: &[SshConnection]) -> Vec<Command> {
    sessions
        .iter()
        .map(|session| {
            let connection = session.clone();
            let name = connection.name.clone();
            Command::new(
                format!("ssh: {name}"),
                [],
                Origin::Remote,
                format!("Open an SSH session to {name}"),
                move |_, py: &mut Pyonji, _: &mut Window, cx: &mut Context<Pyonji>| {
                    py.create_remote_session(&connection, None, None, cx).log();
                },
            )
        })
        .collect()
}

/// The words on a command line, as the values a registered callback takes.
///
/// A command line carries words, not typed values, so every one of them crosses
/// into Lua as a string. The `unwrap_or` is only there because the conversion
/// is fallible in general: a word that somehow failed arrives as `nil`, which
/// is what Lua sees for a missing argument anyway.
fn words_as_lua(lua: &mlua::Lua, args: &[String]) -> Vec<mlua::Value> {
    use mlua::prelude::*;

    args.iter()
        .map(|word| word.clone().into_lua(lua).unwrap_or(mlua::Value::Nil))
        .collect()
}

fn from_lua_actions(actions: &[LuaAction]) -> Vec<Command> {
    use mlua::prelude::*;

    actions
        .iter()
        .map(|action| {
            // The placeholder names come from introspecting the Lua function,
            // so the list shows what the callback actually takes.
            let mut args = action.args.iter().map(Arg::new).collect::<Vec<_>>();
            if action.is_var_arg {
                args.push(Arg::new("..."));
            }
            let callback = action.callback.clone();
            Command::new(
                action.name.clone(),
                args,
                Origin::Config,
                "Runs the callback you registered under this name",
                move |args, py: &mut Pyonji, window: &mut Window, cx: &mut Context<Pyonji>| {
                    let lua = py.lua.clone();
                    let words = LuaMultiValue::from_vec(words_as_lua(&lua, args));
                    if let Err(error) =
                        config::with_env(py, window, cx, |_| callback.call::<LuaValue>(words))
                    {
                        window.dispatch_action(Box::new(crate::PushError::new(error)), cx);
                    }
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands() -> Vec<Command> {
        vec![
            Command::new(
                "switch",
                [Arg::new("tab")],
                Origin::Workspace,
                "Go to a tab",
                |_, _, _, _| {},
            ),
            Command::new(
                "sessions",
                [],
                Origin::Workspace,
                "Pick a running session",
                |_, _, _, _| {},
            ),
            Command::new(
                "ssh",
                [Arg::new("session")],
                Origin::Workspace,
                "Open an SSH session",
                |_, _, _, _| {},
            ),
            Command::new(
                "ssh: server",
                [],
                Origin::Remote,
                "Open an SSH session to server",
                |_, _, _, _| {},
            ),
            Command::new(
                "open",
                [],
                Origin::Config,
                "Open a path in a new tab",
                |_, _, _, _| {},
            ),
        ]
    }

    /// The names a query leaves on the list, in rank order.
    fn names(commands: &[Command], query: &str) -> Vec<String> {
        filter(commands, query)
            .into_iter()
            .map(|entry| entry.command.name)
            .collect()
    }

    #[test]
    fn an_empty_query_offers_everything_in_order() {
        let commands = commands();
        assert_eq!(
            names(&commands, ""),
            ["switch", "sessions", "ssh", "ssh: server", "open"]
        );
        assert_eq!(
            names(&commands, "   "),
            ["switch", "sessions", "ssh", "ssh: server", "open"]
        );
    }

    #[test]
    fn the_name_ends_at_the_first_space() {
        let commands = commands();
        assert_eq!(query_name("switch 2"), "switch");
        assert_eq!(names(&commands, "sw 2"), ["switch"]);
        assert_eq!(query_args("switch 2"), ["2"]);
        assert_eq!(query_args("rename foo bar"), ["foo", "bar"]);
        assert!(query_args("sessions").is_empty());
    }

    #[test]
    fn a_prefix_outranks_a_later_match() {
        let commands = commands();
        // `sessions` has no `h` in it, so it is not a match at all.
        assert_eq!(names(&commands, "ssh"), ["ssh", "ssh: server"]);
    }

    #[test]
    fn the_summary_and_the_placeholders_are_searchable() {
        let commands = commands();
        // Only in a summary.
        assert_eq!(names(&commands, "pick"), ["sessions"]);
        // `switch` says it in a placeholder, `open` in its summary.
        assert_eq!(names(&commands, "tab"), ["switch", "open"]);
    }

    #[test]
    fn marks_land_on_the_name_and_merge_where_they_touch() {
        let commands = commands();
        let filtered = filter(&commands, "sitch");
        let name = &filtered[0].command.name;
        let marks = &filtered[0].marks;
        assert_eq!(name, "switch");
        // `s`, then a gap over the `w`, then the rest.
        assert_eq!(
            marks.iter().map(|r| &name[r.clone()]).collect::<Vec<_>>(),
            ["s", "itch"]
        );
    }

    #[test]
    fn marks_are_byte_ranges_even_when_the_name_is_not_ascii() {
        // `ö` and `ß` are two bytes each, so a character index is not a byte
        // offset: the third character starts at four.
        assert_eq!(byte_ranges("größe", &[0, 3, 4]), [0..1, 4..7]);
        assert_eq!(byte_ranges("größe", &[3, 4]), vec![4..7]);
        assert_eq!(byte_ranges("größe", &[]), Vec::<Range<usize>>::new());
    }

    #[test]
    fn a_summary_match_leaves_the_name_unmarked() {
        let commands = commands();
        let filtered = filter(&commands, "pick");
        assert!(filtered[0].marks.is_empty());
    }

    #[test]
    fn naming_is_stricter_than_the_filter() {
        let commands = commands();
        // Narrows the list by subsequence, but is not the start of a name.
        assert_eq!(names(&commands, "swt"), ["switch"]);
        assert!(!naming(&commands, "swt"));
        // `swi` narrows the list too, but it *is* the start of `switch`.
        assert_eq!(names(&commands, "swi"), ["switch"]);
        assert!(naming(&commands, "swi"));
        // Named, or on the way to it.
        assert!(naming(&commands, "sw"));
        assert!(naming(&commands, "switch"));
        assert!(naming(&commands, "switch 2"));
        assert!(naming(&commands, "SSH: server"));
        // Nothing starts this way.
        assert!(!naming(&commands, "zzz"));
        assert!(!naming(&commands, ""));
    }

    #[test]
    fn nothing_matches_a_nonsense_query() {
        let commands = commands();
        assert!(filter(&commands, "qqqqqq").is_empty());
    }

    #[test]
    fn closest_offers_a_near_miss_as_a_suggestion() {
        let commands = commands();
        assert_eq!(
            closest(&commands, "sesions").map(|c| c.name.as_str()),
            Some("sessions")
        );

        // A swapped pair is one edit, and is the typo people actually make.
        assert_eq!(
            super::closest(&commands, "sessios").map(|c| c.name.as_str()),
            Some("sessions")
        );
        // Too far from anything to guess.
        assert!(super::closest(&commands, "qqqqqq").is_none());
        // Too short to be a near miss of anything.
        assert!(super::closest(&commands, "ss").is_none());
    }

    #[test]
    fn completion_offers_the_best_match_then_the_next() {
        let commands = commands();
        assert_eq!(completion(&commands, "swi", 0).as_deref(), Some("switch"));
        // Both start with what was typed, so both are offered, best first.
        assert_eq!(completion(&commands, "ss", 0).as_deref(), Some("ssh"));
        assert_eq!(
            completion(&commands, "ss", 1).as_deref(),
            Some("ssh: server")
        );
        assert_eq!(completion(&commands, "zzz", 0), None);
    }

    #[test]
    fn completion_keeps_the_arguments_already_typed() {
        let commands = commands();
        assert_eq!(
            completion(&commands, "sw 2", 0).as_deref(),
            Some("switch 2")
        );
        // Nothing left to add.
        assert_eq!(completion(&commands, "switch", 0), None);
    }

    #[test]
    fn usage_reads_like_a_command_line() {
        assert_eq!(commands()[0].usage(), "switch <tab>");
        assert_eq!(commands()[1].usage(), "sessions");
    }

    #[test]
    fn edit_distance_charges_a_swap_once() {
        assert_eq!(edit_distance("swich", "switch"), 1);
        assert_eq!(edit_distance("sesions", "sessions"), 1);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", ""), 3);
    }
}
