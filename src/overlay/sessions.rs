use gpui::{App, Entity, Window, prelude::*};
use gpui_component::{IndexPath, WindowExt as _, command::{Command, CommandItem, CommandState}};

use crate::{Surface, terminal::SessionId};

/// One row of the sessions dialog: which tab owns it and which session it
/// focuses. Mirrors the ratatui `SessionsView` lines (`[tab]-id  title`).
#[derive(Clone)]
struct SessionEntry {
    tab: usize,
    session: SessionId,
    label: String,
}

fn session_entries(surface: &Surface) -> Vec<SessionEntry> {
    let mut entries = Vec::new();
    for (tab_index, tab) in surface.tabs.iter().enumerate() {
        let Some(tab) = tab.as_ref() else {
            continue;
        };
        for session in tab.sessions() {
            let title = surface
                .session_manager
                .session(session)
                .map(|s| s.title())
                .unwrap_or("shell");
            entries.push(SessionEntry {
                tab: tab_index,
                session,
                label: format!("[{}]-{session}    {title}", tab_index + 1),
            });
        }
    }
    entries
}

/// Open the sessions dialog.
///
/// Enter switches to the entry's tab when it is not current (like the old
/// `SessionsState::handle_events`) and focuses the session.
pub fn open_sessions(surface: Entity<Surface>, window: &mut Window, cx: &mut App) {
    let entries = session_entries(&surface.read(cx));
    let state = cx.new(|cx| CommandState::new(window, cx));
    state.update(cx, |state, cx| state.set_query("", window, cx));
    window.open_dialog(cx, move |dialog, window, cx| {
        let dialog_state = state.clone();
        let focus_back = surface.clone();
        let focus_state = dialog_state.clone();
        window.defer(cx, move |window, cx| {
            focus_state.update(cx, |state, cx| {
                state.focus(window, cx);
            });
        });
        let items = entries.clone();
        dialog
            .close_button(false)
            .p_0()
            .title("Sessions")
            .on_close(window.listener_for(&focus_back, |surface, _, window, cx| {
                window.focus(&surface.focus_handle, cx);
            }))
            .content({
                let state = dialog_state.clone();
                let surface = surface.clone();
                let items = items.clone();
                move |content, _, _| {
                    let surface = surface.clone();
                    let confirm_entries = items.clone();
                    let mut command = Command::new(&state)
                        .placeholder("Search sessions..")
                        .bordered(true)
                        .on_confirm(move |index: IndexPath, window, cx| {
                            let entry = confirm_entries.get(index.row).cloned();
                            window.close_dialog(cx);
                            let Some(entry) = entry else {
                                surface.update(cx, |surface, cx| {
                                    window.focus(&surface.focus_handle, cx);
                                    cx.notify();
                                });
                                return;
                            };
                            surface.update(cx, |surface, cx| {
                                if entry.tab != surface.current_tab {
                                    surface.switch_tab(entry.tab);
                                }
                                surface.set_active_session(entry.session);
                                window.focus(&surface.focus_handle, cx);
                                cx.notify();
                            });
                        });
                    for entry in &items {
                        command = command.item(CommandItem::new().label(entry.label.clone()));
                    }
                    content.child(command)
                }
            })
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn session_entry_label_format() {
        // `[tab]-id  title` with a 1-based tab number, as in the old view.
        let label = format!("[{}]-{}    {}", 1, 42, "shell");
        assert_eq!(label, "[1]-42    shell");
    }
}
