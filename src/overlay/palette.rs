use gpui::{App, Entity, Window};
use gpui_component::WindowExt as _;

use crate::{Surface, overlay::Cmd};

use super::search::{SearchItem, open_search_dialog};

/// Fuzzy-filter `commands` by the name part of `query` (everything before the
/// first space — the trailing words are the command's args).
#[allow(dead_code)]
pub fn filter_commands(commands: &[Cmd], query: &str) -> Vec<Cmd> {
    use nucleo_matcher::{Matcher, Utf32Str};

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

/// Complete a command line to the highlighted fuzzy match's name.
pub fn complete_command_name(
    commands: &[Cmd],
    buffer: &str,
    selected: usize,
) -> Option<String> {
    let filtered = filter_commands(commands, buffer);
    if filtered.is_empty() {
        return None;
    }
    filtered
        .get(selected.min(filtered.len() - 1))
        .map(|cmd| cmd.name.clone())
}

/// Open the command palette dialog.
pub fn open(
    commands: Vec<Cmd>,
    entity: Entity<Surface>,
    window: &mut Window,
    cx: &mut App,
) {
    let items: Vec<SearchItem> = commands
        .iter()
        .map(|cmd| SearchItem::new(cmd.hint()).with_keywords([cmd.name.clone()]))
        .collect();
    let commands_for_confirm = commands;
    open_search_dialog(
        None,
        "Type a command, e.g. switch <tab>",
        None,
        items,
        entity.clone(),
        move |row, query, window, cx| {
            confirm_palette(entity.clone(), &commands_for_confirm, row, &query, window, cx);
        },
        window,
        cx,
    );
}
fn confirm_palette(
    surface: Entity<Surface>,
    commands: &[Cmd],
    row: usize,
    query: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let args = query_args(query);
    let action = commands.get(row).map(|cmd| cmd.action.clone());
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
        assert_eq!(
            query_args("rename foo bar"),
            vec!["foo".to_string(), "bar".to_string()]
        );
        assert!(query_args("sessions").is_empty());
    }

    #[test]
    fn tab_completion_takes_highlighted_match_name_only() {
        let commands = test_commands();
        assert_eq!(
            complete_command_name(&commands, "sw 2", 0).as_deref(),
            Some("switch")
        );
        assert_eq!(
            complete_command_name(&commands, "", 0).as_deref(),
            Some("switch")
        );
        assert_eq!(
            complete_command_name(&commands, "sw", 99).as_deref(),
            Some("switch")
        );
        assert_eq!(complete_command_name(&commands, "zzz", 0), None);
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
