use gpui::{App, Entity, Window, div, prelude::*};
use gpui_component::{ActiveTheme as _, IndexPath, WindowExt as _, command::{Command, CommandItem, CommandState}};

use crate::{Surface, terminal::SessionId};

/// One row of the attach dialog: `id  title`, like the old `DetachedView`.
#[derive(Clone)]
pub(crate) struct DetachedEntry {
    session: SessionId,
    label: String,
}

pub(crate) fn detached_entries(surface: &Surface) -> Vec<DetachedEntry> {
    surface
        .live_detached_sessions()
        .into_iter()
        .filter_map(|id| {
            let session = surface.session_manager.session(id)?;
            Some(DetachedEntry {
                session: id,
                label: format!("{id}    {}", session.title()),
            })
        })
        .collect()
}

/// Open the attach-session dialog.
///
/// Enter attaches the highlighted session to the current tab — the same as
/// the old `DetachedState` Enter path (`target = app.current_tab`).
///
/// Note: the ratatui version also accepted `1`–`9` to pick the target tab.
/// That conflicts with the search field (digits must be typeable), so it is
/// intentionally not carried over. Attach lands on the current tab; use the
/// `move-to` command afterwards to relocate the session.
pub fn open_detached(
    entries: Vec<DetachedEntry>,
    current_tab: usize,
    entity: Entity<Surface>,
    window: &mut Window,
    cx: &mut App,
) {
    let state = cx.new(|cx| CommandState::new(window, cx));
    state.update(cx, |state, cx| state.set_query("", window, cx));
    window.open_dialog(cx, move |dialog, window, cx| {
        let dialog_state = state.clone();
        let focus_back = entity.clone();
        let focus_state = dialog_state.clone();
        window.defer(cx, move |window, cx| {
            focus_state.update(cx, |state, cx| {
                state.focus(window, cx);
            });
        });
        let footer = div()
            .w_full()
            .px_3()
            .py_2()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child("Enter attaches to the current tab");
        let items = entries.clone();
        dialog
            .close_button(false)
            .p_0()
            .title("Attach Session")
            .footer(footer)
            .on_close(window.listener_for(&focus_back, |surface, _, window, cx| {
                window.focus(&surface.focus_handle, cx);
            }))
            .content({
                let state = dialog_state.clone();
                let surface = entity.clone();
                let items = items.clone();
                move |content, _, _| {
                    let surface = surface.clone();
                    let confirm_entries = items.clone();
                    let mut command = Command::new(&state)
                        .placeholder("Search detached sessions..")
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
                                surface.reattach_session(entry.session, current_tab);
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
    fn detached_label_format() {
        let label = format!("{}    {}", 7, "nvim");
        assert_eq!(label, "7    nvim");
    }
}
