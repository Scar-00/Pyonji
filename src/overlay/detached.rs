use gpui::{App, Entity, Window};
use gpui_component::WindowExt as _;

use crate::{Surface, terminal::SessionId};

use super::search::{SearchItem, open_search_dialog};

/// One row of the attach dialog: `id  title`, like the old `DetachedView`.
#[derive(Clone)]
pub(crate) struct DetachedEntry {
    session: SessionId,
    label: String,
}

pub(crate) fn detached_entries(workspace: &crate::workspace::Workspace) -> Vec<DetachedEntry> {
    workspace
        .live_detached_sessions()
        .into_iter()
        .filter_map(|id| {
            let session = workspace.session_manager.session(id)?;
            Some(DetachedEntry {
                session: id,
                label: format!("{id}    {}", session.title()),
            })
        })
        .collect()
}

/// Open the attach-session dialog.
///
/// Enter attaches the highlighted session to the current tab.
pub fn open(
    entries: Vec<DetachedEntry>,
    current_tab: usize,
    entity: Entity<Surface>,
    window: &mut Window,
    cx: &mut App,
) {
    let items: Vec<SearchItem> = entries
        .iter()
        .map(|entry| SearchItem::new(entry.label.clone()))
        .collect();
    open_search_dialog(
        Some("Attach Session".into()),
        "Search detached sessions..",
        Some("Enter attaches to the current tab"),
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
                surface.reattach_session(entry.session, current_tab);
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
    fn detached_label_format() {
        let label = format!("{}    {}", 7, "nvim");
        assert_eq!(label, "7    nvim");
    }
}
