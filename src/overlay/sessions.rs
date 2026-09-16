use gpui::{App, Entity, Window};
use gpui_component::WindowExt as _;

use crate::{Surface, terminal::SessionId};

use super::search::{SearchItem, open_search_dialog};

/// One row of the sessions dialog: which tab owns it and which session it
/// focuses. Mirrors the ratatui `SessionsView` lines (`[tab]-id  title`).
#[derive(Clone)]
pub(crate) struct SessionEntry {
    tab: usize,
    session: SessionId,
    label: String,
}

pub(crate) fn session_entries(workspace: &crate::workspace::Workspace) -> Vec<SessionEntry> {
    let mut entries = Vec::new();
    for (tab_index, tab) in workspace.tabs.iter().enumerate() {
        let Some(tab) = tab.as_ref() else {
            continue;
        };
        for session in tab.sessions() {
            let title = workspace
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
pub fn open(
    entries: Vec<SessionEntry>,
    entity: Entity<Surface>,
    window: &mut Window,
    cx: &mut App,
) {
    let items: Vec<SearchItem> = entries
        .iter()
        .map(|entry| SearchItem::new(entry.label.clone()))
        .collect();
    open_search_dialog(
        Some("Sessions".into()),
        "Search sessions..",
        None,
        items,
        entity.clone(),
        move |row, _, window, cx| {
            let entry = entries.get(row).cloned();
            window.close_dialog(cx);
            let Some(entry) = entry else {
                entity.update(cx, |surface, cx| {
                    window.focus(&surface.focus_handle, cx);
                    cx.notify();
                });
                return;
            };
            entity.update(cx, |surface, cx| {
                if entry.tab != surface.workspace.current_tab {
                    surface.switch_tab(entry.tab);
                }
                surface.workspace.set_active_session(entry.session);
                window.focus(&surface.focus_handle, cx);
                cx.notify();
            });
        },
        window,
        cx,
    );
}
#[cfg(test)]
mod tests {
    #[test]
    fn session_entry_label_format() {
        let label = format!("[{}]-{}    {}", 1, 42, "shell");
        assert_eq!(label, "[1]-42    shell");
    }
}
