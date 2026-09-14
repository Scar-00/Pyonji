use gpui::{App, Entity, Window, prelude::*};
use gpui_component::{IndexPath, WindowExt as _, command::{Command, CommandItem, CommandState}};
use nucleo_matcher::{Matcher, Utf32Str};

use crate::{Surface, overlay::Cmd};

/// Fuzzy-filter `commands` by the name part of `query` (everything before the
/// first space — the trailing words are the command's args).
///
/// Same behavior as the previous palette: empty query matches everything,
/// otherwise nucleo fuzzy-match on the command name, best score first.
///
/// The dialog itself filters through the GPUI `Command` view; this stays as
/// the canonical, unit-tested matcher for the name/args query shape.
#[allow(dead_code)]
pub fn filter_commands(commands: &[Cmd], query: &str) -> Vec<Cmd> {
    let (name, _) = query.split_once(' ').unwrap_or((query, ""));
    if name.is_empty() {
        return commands.to_vec();
    }
    let mut matcher = Matcher::default();
    let mut buf_1 = vec![];
    let mut buf_2 = vec![];
    let mut scores = commands
        .iter()
        .filter_map(|cmd| {
            Some((
                cmd,
                matcher.fuzzy_match(
                    Utf32Str::new(&cmd.name, &mut buf_1),
                    Utf32Str::new(name, &mut buf_2),
                )?,
            ))
        })
        .collect::<Vec<_>>();
    scores.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    scores.into_iter().map(|(cmd, _)| cmd.clone()).collect()
}

/// Split a palette query into the selected command's args: everything after
/// the first space.
pub fn query_args(query: &str) -> Vec<String> {
    let mut split = query.split(' ');
    split.next();
    split.map(str::to_string).collect()
}

/// Open the command palette dialog.
///
/// Typing filters the commands; Enter runs the highlighted one with the
/// trailing query words as args (e.g. `switch 2`).
///
/// Note: the ratatui palette's Tab-to-complete-name has no equivalent in the
/// GPUI `Command` view, so it is intentionally not carried over. Everything
/// else — fuzzy filtering, `<arg>` hints, arg parsing — is preserved.
pub fn open_palette(surface: Entity<Surface>, window: &mut Window, cx: &mut App) {
    let commands = surface.read(cx).palette_commands.clone();
    let state = cx.new(|cx| CommandState::new(window, cx));
    state.update(cx, |state, cx| state.set_query("", window, cx));
    window.open_dialog(cx, move |dialog, window, cx| {
        let dialog_state = state.clone();
        let focus_back = surface.clone();
        // Focus the search field on mount so typing filters the palette
        // instead of being sent to the pty.
        let focus_state = dialog_state.clone();
        window.defer(cx, move |window, cx| {
            focus_state.update(cx, |state, cx| {
                state.focus(window, cx);
            });
        });
        // Cloned per build: the dialog builder is `Fn`, so nothing may move
        // out of the captured environment.
        let items = commands.clone();
        dialog
            .close_button(false)
            .p_0()
            .on_close(window.listener_for(&focus_back, |surface, _, window, cx| {
                window.focus(&surface.focus_handle, cx);
            }))
            .content({
                let state = dialog_state.clone();
                let surface = surface.clone();
                let items = items.clone();
                move |content, _, _| {
                    let state_for_confirm = state.clone();
                    let surface = surface.clone();
                    let mut command = Command::new(&state)
                        .placeholder("Type a command, e.g. switch <tab>")
                        .bordered(true)
                        .on_confirm(move |index: IndexPath, window, cx| {
                            confirm_palette(
                                surface.clone(),
                                state_for_confirm.clone(),
                                index,
                                window,
                                cx,
                            );
                        });
                    for cmd in &items {
                        let label = cmd.hint();
                        command = command.item(
                            CommandItem::new()
                                .label(label)
                                .keywords([cmd.name.clone()]),
                        );
                    }
                    content.child(command)
                }
            })
    });
}

fn confirm_palette(
    surface: Entity<Surface>,
    state: Entity<CommandState>,
    index: IndexPath,
    window: &mut Window,
    cx: &mut App,
) {
    let query = state.read(cx).query(cx).to_string();
    let args = query_args(&query);
    let action = surface.read(cx).palette_commands.get(index.row).map(|cmd| cmd.action.clone());
    window.close_dialog(cx);
    let Some(action) = action else {
        surface.update(cx, |surface, cx| {
            window.focus(&surface.focus_handle, cx);
            cx.notify();
        });
        return;
    };
    action(surface.clone(), window, cx, args);
    surface.update(cx, |surface, cx| {
        window.focus(&surface.focus_handle, cx);
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    fn test_commands() -> Vec<Cmd> {
        vec![
            Cmd::new("switch", [crate::overlay::Arg::new("tab")], |_, _, _, _| {}),
            Cmd::new("sessions", [], |_, _, _, _| {}),
            Cmd::new("ssh", [crate::overlay::Arg::new("session")], |_, _, _, _| {}),
        ]
    }

    #[test]
    fn empty_query_matches_everything() {
        let commands = test_commands();
        assert_eq!(filter_commands(&commands, "").len(), 3);
        assert_eq!(filter_commands(&commands, "   ").len(), 3);
    }

    #[test]
    fn fuzzy_match_orders_best_first() {
        let commands = test_commands();
        let filtered = filter_commands(&commands, "sw");
        assert_eq!(filtered.first().map(|cmd| cmd.name.as_str()), Some("switch"));
        assert!(filter_commands(&commands, "zzz").is_empty());
    }

    #[test]
    fn query_args_skips_command_name() {
        assert_eq!(query_args("switch 2"), vec!["2".to_string()]);
        assert_eq!(query_args("rename foo bar"), vec!["foo".to_string(), "bar".to_string()]);
        assert!(query_args("sessions").is_empty());
    }

    #[test]
    fn rc_clone_keeps_action() {
        let called = Rc::new(std::cell::Cell::new(false));
        let flag = called.clone();
        let cmd = Cmd::new("demo", [], move |_, _, _, _| flag.set(true));
        assert_eq!(cmd.hint(), "demo");
        let _ = (cmd, called);
    }
}
